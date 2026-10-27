//! The synthetic-data confirmation for cloud judges (`h2w_api::judge`): what the places that
//! start judging (the report tab, the pipeline wizard) ask before they send a job.

use h2w_api::{CloudJudgePolicy, JudgeSite};

/// What starting a job that judges needs, given where the profile's judge runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JudgeGate {
    /// No judge model, or a local judge: nothing to confirm.
    Open,
    /// A cloud judge, or one the server cannot place: the user confirms that the
    /// experiment's tasks and outputs are generated or synthetic data.
    Confirm,
    /// A cloud judge on a server that refuses them (`[judge] cloud = "refuse"`).
    Refused,
}

impl JudgeGate {
    pub(crate) fn of(site: Option<&JudgeSite>, policy: CloudJudgePolicy) -> JudgeGate {
        match site {
            Some(s) if s.needs_acknowledgement() => match policy {
                CloudJudgePolicy::Acknowledge => JudgeGate::Confirm,
                CloudJudgePolicy::Refuse => JudgeGate::Refused,
            },
            _ => JudgeGate::Open,
        }
    }

    /// Whether a job that judges may start with this confirmation state.
    pub(crate) fn allows(self, confirmed: bool) -> bool {
        match self {
            JudgeGate::Open => true,
            JudgeGate::Confirm => confirmed,
            JudgeGate::Refused => false,
        }
    }

    /// The `acknowledge_cloud_judge` to send: only a confirmation the user gave, and only
    /// for a judge that needs it.
    pub(crate) fn acknowledgement(self, confirmed: bool) -> bool {
        self == JudgeGate::Confirm && confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::JudgeSiteKind;

    fn site(kind: JudgeSiteKind) -> JudgeSite {
        JudgeSite {
            kind,
            provider: None,
            hosts: vec![],
        }
    }

    #[test]
    fn local_judges_are_open_and_cloud_ones_ask_or_are_refused() {
        let ack = CloudJudgePolicy::Acknowledge;
        let refuse = CloudJudgePolicy::Refuse;
        assert_eq!(JudgeGate::of(None, ack), JudgeGate::Open);
        let local = site(JudgeSiteKind::Local);
        assert_eq!(JudgeGate::of(Some(&local), refuse), JudgeGate::Open);
        for kind in [JudgeSiteKind::Cloud, JudgeSiteKind::Unknown] {
            assert_eq!(JudgeGate::of(Some(&site(kind)), ack), JudgeGate::Confirm);
            assert_eq!(JudgeGate::of(Some(&site(kind)), refuse), JudgeGate::Refused);
        }
    }

    #[test]
    fn only_a_given_confirmation_is_sent() {
        assert!(JudgeGate::Open.allows(false));
        assert!(!JudgeGate::Open.acknowledgement(true));
        assert!(!JudgeGate::Confirm.allows(false));
        assert!(JudgeGate::Confirm.allows(true));
        assert!(JudgeGate::Confirm.acknowledgement(true));
        assert!(!JudgeGate::Confirm.acknowledgement(false));
        assert!(!JudgeGate::Refused.allows(true));
        assert!(!JudgeGate::Refused.acknowledgement(true));
    }
}
