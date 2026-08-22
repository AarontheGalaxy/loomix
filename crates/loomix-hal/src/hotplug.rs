//! Hot-plug and auto-restart decisions (spec 3.4 M4; spec 1.10's two
//! independent menu toggles, "auto restart audio engine when the A1
//! device disconnects" and "...when any device disconnects"; spec 1.19,
//! "when a device disappears the engine stops"). Pure: given the current
//! configuration and one hot-plug event, decides whether the engine
//! should restart, nothing about how to actually tear down or rebuild it.
//!
//! Clock-master re-resolution after a restart is `clock::resolve_clock_source`'s
//! job, not duplicated here.

use crate::clock::DeviceId;

#[derive(Debug, Clone, Copy, Default)]
pub struct HotplugConfig {
    pub restart_on_a1_disconnect: bool,
    pub restart_on_any_disconnect: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    DeviceRemoved(DeviceId),
    DeviceAdded(DeviceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Restart,
    NoOp,
}

/// `a1_device` is the currently configured main-output device, if any
/// (spec 1.11) -- the one whose disconnect the A1-specific toggle cares
/// about, and the one whose reappearance the engine should reattach to.
pub fn decide(config: &HotplugConfig, a1_device: Option<DeviceId>, event: Event) -> Action {
    match event {
        Event::DeviceRemoved(id) => {
            let is_a1 = a1_device == Some(id);
            let should_restart =
                config.restart_on_any_disconnect || (is_a1 && config.restart_on_a1_disconnect);
            if should_restart {
                Action::Restart
            } else {
                Action::NoOp
            }
        }
        Event::DeviceAdded(id) => {
            // The configured main-output device coming back is picked up
            // unconditionally: the disconnect toggles gate whether *losing*
            // it needed a restart, not whether *getting it back* does. An
            // unrelated device appearing needs no action -- it's just now
            // available for the user to select later.
            if a1_device == Some(id) {
                Action::Restart
            } else {
                Action::NoOp
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A1: DeviceId = 1;
    const OTHER: DeviceId = 2;

    /// Every (restart_on_a1, restart_on_any, event) combination that
    /// matters, exhaustively -- the same reduction M3's routing truth
    /// table uses, not hand-picked cases.
    #[test]
    fn transition_table() {
        struct Case {
            restart_on_a1: bool,
            restart_on_any: bool,
            event: Event,
            expected: Action,
        }
        let cases = [
            // Nothing enabled: never restart on disconnect, but the
            // configured device reappearing is always reattached.
            Case {
                restart_on_a1: false,
                restart_on_any: false,
                event: Event::DeviceRemoved(A1),
                expected: Action::NoOp,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: false,
                event: Event::DeviceRemoved(OTHER),
                expected: Action::NoOp,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: false,
                event: Event::DeviceAdded(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: false,
                event: Event::DeviceAdded(OTHER),
                expected: Action::NoOp,
            },
            // A1-only: restarts on A1 disconnect, not on some other
            // device's.
            Case {
                restart_on_a1: true,
                restart_on_any: false,
                event: Event::DeviceRemoved(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: false,
                event: Event::DeviceRemoved(OTHER),
                expected: Action::NoOp,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: false,
                event: Event::DeviceAdded(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: false,
                event: Event::DeviceAdded(OTHER),
                expected: Action::NoOp,
            },
            // Any-only: restarts regardless of which device disconnected.
            Case {
                restart_on_a1: false,
                restart_on_any: true,
                event: Event::DeviceRemoved(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: true,
                event: Event::DeviceRemoved(OTHER),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: true,
                event: Event::DeviceAdded(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: false,
                restart_on_any: true,
                event: Event::DeviceAdded(OTHER),
                expected: Action::NoOp,
            },
            // Both enabled: same as any-only for disconnects, since
            // any-only already covers the A1 case.
            Case {
                restart_on_a1: true,
                restart_on_any: true,
                event: Event::DeviceRemoved(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: true,
                event: Event::DeviceRemoved(OTHER),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: true,
                event: Event::DeviceAdded(A1),
                expected: Action::Restart,
            },
            Case {
                restart_on_a1: true,
                restart_on_any: true,
                event: Event::DeviceAdded(OTHER),
                expected: Action::NoOp,
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let config = HotplugConfig {
                restart_on_a1_disconnect: case.restart_on_a1,
                restart_on_any_disconnect: case.restart_on_any,
            };
            let action = decide(&config, Some(A1), case.event);
            assert_eq!(
                action, case.expected,
                "case {i}: restart_on_a1={}, restart_on_any={}, event={:?}",
                case.restart_on_a1, case.restart_on_any, case.event
            );
        }
    }

    #[test]
    fn no_a1_configured_means_nothing_can_match_it() {
        let config = HotplugConfig {
            restart_on_a1_disconnect: true,
            restart_on_any_disconnect: false,
        };
        // No main-output device configured at all (spec 1.11's "running
        // with no output device"): a disconnect can't be "the A1 device"
        // if there isn't one, and a reconnect has nothing configured to
        // match either.
        assert_eq!(
            decide(&config, None, Event::DeviceRemoved(A1)),
            Action::NoOp
        );
        assert_eq!(decide(&config, None, Event::DeviceAdded(A1)), Action::NoOp);
    }
}
