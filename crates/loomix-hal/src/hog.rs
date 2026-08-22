//! Hog-mode acquisition decisions (spec 1.11's "exclusive mode toggle for
//! inputs"; spec 2.2 maps it to `kAudioDevicePropertyHogMode`, whose value
//! is the pid currently holding exclusive access, or none). Pure: given
//! whether exclusive access is wanted and who (if anyone) currently holds
//! it, decides what to do -- not how to actually read or write the
//! CoreAudio property.

pub type Pid = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HogDecision {
    /// Free -- attempt to acquire.
    Acquire,
    /// Already ours -- nothing to do.
    AlreadyOurs,
    /// Held by another process -- exclusive access can't be granted. The
    /// caller falls back to shared (non-exclusive) mode rather than
    /// failing the device open outright: hog mode is "the same idea" as
    /// WASAPI exclusive (spec 2.2), not a hard requirement to run at all.
    HeldByAnother(Pid),
    /// We hold it but no longer want it -- release.
    Release,
    /// Not held and not wanted -- nothing to do.
    NoActionNeeded,
}

pub fn decide(requested: bool, current_owner: Option<Pid>, our_pid: Pid) -> HogDecision {
    match (requested, current_owner) {
        (true, None) => HogDecision::Acquire,
        (true, Some(pid)) if pid == our_pid => HogDecision::AlreadyOurs,
        (true, Some(pid)) => HogDecision::HeldByAnother(pid),
        (false, Some(pid)) if pid == our_pid => HogDecision::Release,
        (false, _) => HogDecision::NoActionNeeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: Pid = 100;
    const THEM: Pid = 200;

    #[test]
    fn transition_table() {
        let cases: &[((bool, Option<Pid>), HogDecision)] = &[
            ((true, None), HogDecision::Acquire),
            ((true, Some(US)), HogDecision::AlreadyOurs),
            ((true, Some(THEM)), HogDecision::HeldByAnother(THEM)),
            ((false, None), HogDecision::NoActionNeeded),
            ((false, Some(US)), HogDecision::Release),
            ((false, Some(THEM)), HogDecision::NoActionNeeded),
        ];
        for ((requested, owner), expected) in cases {
            assert_eq!(
                decide(*requested, *owner, US),
                *expected,
                "requested={requested}, owner={owner:?}"
            );
        }
    }
}
