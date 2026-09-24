//! Mission-wide call/deadline gates. Reference counts are supplied by the frozen
//! tokenizer; provider telemetry must never be substituted for these counts.
use myr_core::FailCode;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub calls: u32,
    pub reference_tokens: u64,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Ledger {
    pub calls_started: u32,
    pub input_reference_tokens: u64,
    pub output_reference_tokens: u64,
    /// Reserved output for a call whose complete output was not recovered.
    pub uncertain_output_reservation: u64,
    pub output_accounting_complete: bool,
}

/// A permit authorizes one request only. No Clone/Deserialize implementation:
/// callers cannot replay a permit or forge one from provider output.
#[derive(Debug)]
pub struct Permit {
    id: u32,
    owner: Arc<()>,
    timeout: Duration,
    output_reference_allowance: u64,
}

impl Permit {
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    pub fn output_reference_allowance(&self) -> u64 {
        self.output_reference_allowance
    }
}

pub struct Budget {
    identity: Arc<()>,
    limits: Limits,
    start: Instant,
    ledger: Ledger,
    pending: Option<u32>,
    stopped: Option<FailCode>,
}

impl Budget {
    pub fn new(limits: Limits) -> Result<Self, FailCode> {
        let start = Instant::now();
        if limits.calls == 0
            || limits.reference_tokens == 0
            || limits.elapsed.is_zero()
            || start.checked_add(limits.elapsed).is_none()
        {
            return Err(FailCode::InvalidGoal);
        }
        Ok(Self {
            identity: Arc::new(()),
            limits,
            start,
            ledger: Ledger {
                calls_started: 0,
                input_reference_tokens: 0,
                output_reference_tokens: 0,
                uncertain_output_reservation: 0,
                output_accounting_complete: true,
            },
            pending: None,
            stopped: None,
        })
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    pub fn deadline(&self) -> Instant {
        self.start
            .checked_add(self.limits.elapsed)
            .expect("validated deadline")
    }

    /// Charge input and one call before dispatch, reserving space for output.
    /// Retries and repair calls must request a fresh permit. A single mission
    /// owns this gate across every role, so role changes cannot reset limits.
    pub fn reserve(&mut self, input: u64, requested_output: u64) -> Result<Permit, FailCode> {
        self.reserve_at(input, requested_output, Instant::now())
    }

    fn reserve_at(
        &mut self,
        input: u64,
        requested_output: u64,
        now: Instant,
    ) -> Result<Permit, FailCode> {
        if let Some(code) = self.stopped {
            return Err(code);
        }
        if self.pending.is_some() {
            return Err(FailCode::RuntimeError);
        }
        let elapsed = now
            .checked_duration_since(self.start)
            .ok_or(FailCode::RuntimeError)?;
        if elapsed >= self.limits.elapsed {
            return Err(self.stop(FailCode::TimeBudget));
        }
        if self.ledger.calls_started >= self.limits.calls {
            return Err(self.stop(FailCode::CallBudget));
        }
        let remaining = self
            .limits
            .reference_tokens
            .saturating_sub(self.ledger.input_reference_tokens)
            .saturating_sub(self.ledger.output_reference_tokens);
        if requested_output == 0 || input >= remaining {
            return Err(self.stop(FailCode::TokenBudget));
        }
        let allowance = requested_output.min(remaining - input);
        self.ledger.calls_started += 1;
        self.ledger.input_reference_tokens += input;
        self.pending = Some(self.ledger.calls_started);
        self.ledger.uncertain_output_reservation = allowance;
        self.ledger.output_accounting_complete = false;
        Ok(Permit {
            owner: Arc::clone(&self.identity),
            id: self.ledger.calls_started,
            timeout: self.limits.elapsed - elapsed,
            output_reference_allowance: allowance,
        })
    }

    /// Settle even refused, malformed, or failed responses. None means complete
    /// reference-token usage cannot be established (for example, lost output on
    /// timeout); stop further requests instead of inventing a zero usage count.
    /// Native provider counts belong in separate transport telemetry.
    pub fn settle(&mut self, permit: Permit, output: Option<u64>) -> Result<(), FailCode> {
        self.settle_at(permit, output, Instant::now())
    }

    fn settle_at(
        &mut self,
        permit: Permit,
        output: Option<u64>,
        now: Instant,
    ) -> Result<(), FailCode> {
        if !Arc::ptr_eq(&self.identity, &permit.owner) || self.pending != Some(permit.id) {
            return Err(self.stop(FailCode::RuntimeError));
        }
        self.pending = None;
        let Some(output) = output else {
            return Err(self.stop(FailCode::TokenBudget));
        };
        self.ledger.output_reference_tokens =
            match self.ledger.output_reference_tokens.checked_add(output) {
                Some(value) => value,
                None => return Err(self.stop(FailCode::TokenBudget)),
            };
        self.ledger.uncertain_output_reservation = 0;
        self.ledger.output_accounting_complete = true;
        if now
            .checked_duration_since(self.start)
            .ok_or(FailCode::RuntimeError)?
            >= self.limits.elapsed
        {
            return Err(self.stop(FailCode::TimeBudget));
        }
        if output > permit.output_reference_allowance {
            return Err(self.stop(FailCode::TokenBudget));
        }
        Ok(())
    }

    fn stop(&mut self, code: FailCode) -> FailCode {
        self.stopped = Some(code);
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget(calls: u32, tokens: u64) -> Budget {
        Budget::new(Limits {
            calls,
            reference_tokens: tokens,
            elapsed: Duration::from_secs(10),
        })
        .unwrap()
    }
    #[test]
    fn repairs_share_call_limit_and_charge_all_input_and_output() {
        let mut b = budget(2, 100);
        let first = b.reserve(20, 50).unwrap();
        b.settle(first, Some(10)).unwrap();
        let second = b.reserve(20, 100).unwrap();
        assert_eq!(second.output_reference_allowance, 50);
        b.settle(second, Some(5)).unwrap();
        assert!(matches!(b.reserve(1, 1), Err(FailCode::CallBudget)));
        assert_eq!(b.ledger.calls_started, 2);
        assert_eq!(b.ledger.input_reference_tokens, 40);
        assert_eq!(b.ledger.output_reference_tokens, 15);
    }
    #[test]
    fn unknown_usage_retains_reservation_and_prevents_further_calls() {
        let mut b = budget(10, 100);
        let p = b.reserve(20, 30).unwrap();
        assert!(matches!(b.reserve(0, 1), Err(FailCode::RuntimeError)));
        assert_eq!(b.settle(p, None), Err(FailCode::TokenBudget));
        assert!(!b.ledger.output_accounting_complete);
        assert_eq!(b.ledger.uncertain_output_reservation, 30);
        assert!(matches!(b.reserve(0, 1), Err(FailCode::TokenBudget)));
    }
    #[test]
    fn deadline_clamps_timeout_and_late_completion_keeps_usage() {
        let mut b = budget(10, 100);
        let now = b.start + Duration::from_secs(9);
        let p = b.reserve_at(1, 20, now).unwrap();
        assert_eq!(p.timeout, Duration::from_secs(1));
        assert_eq!(
            b.settle_at(p, Some(2), now + Duration::from_secs(1)),
            Err(FailCode::TimeBudget)
        );
        assert_eq!(b.ledger.output_reference_tokens, 2);
        assert!(matches!(b.reserve(1, 1), Err(FailCode::TimeBudget)));
    }
    #[test]
    fn overflow_and_provider_overrun_cannot_reopen_budget() {
        let mut b = budget(10, 100);
        assert!(matches!(b.reserve(u64::MAX, 1), Err(FailCode::TokenBudget)));
        assert_eq!(b.ledger.calls_started, 0);
        let mut b = budget(10, 100);
        let p = b.reserve(10, 20).unwrap();
        assert_eq!(b.settle(p, Some(21)), Err(FailCode::TokenBudget));
        assert_eq!(b.ledger.output_reference_tokens, 21);
        assert!(matches!(b.reserve(1, 1), Err(FailCode::TokenBudget)));
        let mut other = budget(10, 100);
        let foreign = other.reserve(1, 1).unwrap();
        let mut target = budget(10, 100);
        let _pending = target.reserve(1, 1).unwrap();
        assert_eq!(target.settle(foreign, Some(0)), Err(FailCode::RuntimeError));
    }
}
