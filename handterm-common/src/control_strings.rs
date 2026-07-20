#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SixelEvent {
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcsEvent {
    Generic(Vec<u8>),
    Sixel(SixelEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApcEvent {
    Generic(Vec<u8>),
    KittyGraphics(Vec<u8>),
    Latex(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    Raw(Vec<u8>),
    Title { raw: Vec<u8>, title: String },
    Clipboard { raw: Vec<u8>, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlStringEvent {
    Osc(OscEvent),
    Dcs(DcsEvent),
    Apc(ApcEvent),
}

const CONTROL_STRING_EVENT_LIMIT: usize = 256;

fn push_bounded<T>(queue: &mut Vec<T>, value: T) {
    if queue.len() >= CONTROL_STRING_EVENT_LIMIT {
        queue.remove(0);
    }
    queue.push(value);
}

#[derive(Debug, Default)]
pub struct ControlStringState {
    dcs_events: Vec<DcsEvent>,
    sixel_events: Vec<SixelEvent>,
    apc_events: Vec<ApcEvent>,
    osc_events: Vec<OscEvent>,
    control_string_events: Vec<ControlStringEvent>,
}

impl ControlStringState {
    pub fn take_osc(&mut self) -> Option<OscEvent> {
        if self.osc_events.is_empty() {
            None
        } else {
            Some(self.osc_events.remove(0))
        }
    }

    pub fn drain_osc(&mut self) -> Vec<OscEvent> {
        std::mem::take(&mut self.osc_events)
    }

    pub fn take_control_string(&mut self) -> Option<ControlStringEvent> {
        if self.control_string_events.is_empty() {
            None
        } else {
            Some(self.control_string_events.remove(0))
        }
    }

    pub fn drain_control_strings(&mut self) -> Vec<ControlStringEvent> {
        std::mem::take(&mut self.control_string_events)
    }

    pub fn take_dcs(&mut self) -> Option<DcsEvent> {
        if self.dcs_events.is_empty() {
            None
        } else {
            Some(self.dcs_events.remove(0))
        }
    }

    pub fn drain_dcs(&mut self) -> Vec<DcsEvent> {
        std::mem::take(&mut self.dcs_events)
    }

    pub fn take_sixel(&mut self) -> Option<SixelEvent> {
        if self.sixel_events.is_empty() {
            None
        } else {
            Some(self.sixel_events.remove(0))
        }
    }

    pub fn drain_sixel(&mut self) -> Vec<SixelEvent> {
        std::mem::take(&mut self.sixel_events)
    }

    pub fn take_apc(&mut self) -> Option<ApcEvent> {
        if self.apc_events.is_empty() {
            None
        } else {
            Some(self.apc_events.remove(0))
        }
    }

    pub fn drain_apc(&mut self) -> Vec<ApcEvent> {
        std::mem::take(&mut self.apc_events)
    }

    pub fn push_osc(&mut self, event: OscEvent) {
        push_bounded(
            &mut self.control_string_events,
            ControlStringEvent::Osc(event.clone()),
        );
        push_bounded(&mut self.osc_events, event);
    }

    pub fn push_dcs(&mut self, event: DcsEvent) {
        push_bounded(
            &mut self.control_string_events,
            ControlStringEvent::Dcs(event.clone()),
        );
        if let DcsEvent::Sixel(ref sixel) = event {
            push_bounded(&mut self.sixel_events, sixel.clone());
        }
        push_bounded(&mut self.dcs_events, event);
    }

    pub fn push_apc(&mut self, event: ApcEvent) {
        push_bounded(
            &mut self.control_string_events,
            ControlStringEvent::Apc(event.clone()),
        );
        push_bounded(&mut self.apc_events, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_none_when_empty() {
        let mut state = ControlStringState::default();
        assert_eq!(state.take_osc(), None);
        assert_eq!(state.take_dcs(), None);
        assert_eq!(state.take_apc(), None);
        assert_eq!(state.take_sixel(), None);
        assert_eq!(state.take_control_string(), None);
    }

    #[test]
    fn drain_on_empty_state_returns_empty_vecs() {
        let mut state = ControlStringState::default();
        assert!(state.drain_osc().is_empty());
        assert!(state.drain_dcs().is_empty());
        assert!(state.drain_apc().is_empty());
        assert!(state.drain_sixel().is_empty());
        assert!(state.drain_control_strings().is_empty());
    }

    #[test]
    fn osc_events_take_in_fifo_order() {
        let mut state = ControlStringState::default();
        state.push_osc(OscEvent::Raw(b"0;first".to_vec()));
        state.push_osc(OscEvent::Title {
            raw: b"2;second".to_vec(),
            title: "second".to_string(),
        });

        assert_eq!(state.take_osc(), Some(OscEvent::Raw(b"0;first".to_vec())));
        assert_eq!(
            state.take_osc(),
            Some(OscEvent::Title {
                raw: b"2;second".to_vec(),
                title: "second".to_string(),
            })
        );
        assert_eq!(state.take_osc(), None);
    }

    #[test]
    fn drain_returns_all_events_and_empties_queue() {
        let mut state = ControlStringState::default();
        state.push_apc(ApcEvent::Generic(b"a".to_vec()));
        state.push_apc(ApcEvent::KittyGraphics(b"Gf=32".to_vec()));

        let drained = state.drain_apc();
        assert_eq!(
            drained,
            vec![
                ApcEvent::Generic(b"a".to_vec()),
                ApcEvent::KittyGraphics(b"Gf=32".to_vec()),
            ]
        );
        assert!(state.drain_apc().is_empty());
        assert_eq!(state.take_apc(), None);
    }

    #[test]
    fn sixel_dcs_events_are_mirrored_into_sixel_queue() {
        let mut state = ControlStringState::default();
        state.push_dcs(DcsEvent::Generic(b"+q544e".to_vec()));
        state.push_dcs(DcsEvent::Sixel(SixelEvent {
            payload: b"q#0;2;0;0;0".to_vec(),
        }));

        // Only the sixel event lands in the dedicated sixel queue.
        assert_eq!(
            state.take_sixel(),
            Some(SixelEvent {
                payload: b"q#0;2;0;0;0".to_vec(),
            })
        );
        assert_eq!(state.take_sixel(), None);

        // Both remain visible through the generic DCS queue, in order.
        assert_eq!(
            state.drain_dcs(),
            vec![
                DcsEvent::Generic(b"+q544e".to_vec()),
                DcsEvent::Sixel(SixelEvent {
                    payload: b"q#0;2;0;0;0".to_vec(),
                }),
            ]
        );
    }

    #[test]
    fn unified_queue_preserves_cross_type_arrival_order() {
        let mut state = ControlStringState::default();
        state.push_osc(OscEvent::Raw(b"osc".to_vec()));
        state.push_dcs(DcsEvent::Generic(b"dcs".to_vec()));
        state.push_apc(ApcEvent::Generic(b"apc".to_vec()));

        assert_eq!(
            state.drain_control_strings(),
            vec![
                ControlStringEvent::Osc(OscEvent::Raw(b"osc".to_vec())),
                ControlStringEvent::Dcs(DcsEvent::Generic(b"dcs".to_vec())),
                ControlStringEvent::Apc(ApcEvent::Generic(b"apc".to_vec())),
            ]
        );
    }

    #[test]
    fn unified_queue_drains_independently_of_per_type_queues() {
        let mut state = ControlStringState::default();
        state.push_osc(OscEvent::Raw(b"x".to_vec()));

        // Draining the unified queue must not consume the per-type copy.
        assert_eq!(state.drain_control_strings().len(), 1);
        assert_eq!(state.take_osc(), Some(OscEvent::Raw(b"x".to_vec())));

        // And vice versa.
        state.push_osc(OscEvent::Raw(b"y".to_vec()));
        assert_eq!(state.take_osc(), Some(OscEvent::Raw(b"y".to_vec())));
        assert_eq!(
            state.take_control_string(),
            Some(ControlStringEvent::Osc(OscEvent::Raw(b"y".to_vec())))
        );
    }

    #[test]
    fn payload_bytes_survive_verbatim_including_control_bytes() {
        // Payloads are raw bytes: embedded ESC, BEL, NUL, and invalid UTF-8
        // must be preserved untouched, not treated as terminators here.
        let payload = vec![0x1b, 0x07, 0x00, 0xff, b'G', 0x9c];
        let mut state = ControlStringState::default();
        state.push_apc(ApcEvent::KittyGraphics(payload.clone()));
        state.push_osc(OscEvent::Clipboard {
            raw: payload.clone(),
            data: payload.clone(),
        });

        assert_eq!(
            state.take_apc(),
            Some(ApcEvent::KittyGraphics(payload.clone()))
        );
        assert_eq!(
            state.take_osc(),
            Some(OscEvent::Clipboard {
                raw: payload.clone(),
                data: payload,
            })
        );
    }

    #[test]
    fn empty_payloads_are_valid_events() {
        let mut state = ControlStringState::default();
        state.push_osc(OscEvent::Raw(Vec::new()));
        state.push_dcs(DcsEvent::Generic(Vec::new()));
        assert_eq!(state.take_osc(), Some(OscEvent::Raw(Vec::new())));
        assert_eq!(state.take_dcs(), Some(DcsEvent::Generic(Vec::new())));
    }

    #[test]
    fn queues_drop_oldest_events_beyond_capacity() {
        // A misbehaving app spamming control strings must not grow queues
        // unboundedly: the oldest events are evicted, newest kept.
        let mut state = ControlStringState::default();
        let overflow = 10;
        for i in 0..CONTROL_STRING_EVENT_LIMIT + overflow {
            state.push_osc(OscEvent::Raw(i.to_string().into_bytes()));
        }

        let events = state.drain_osc();
        assert_eq!(events.len(), CONTROL_STRING_EVENT_LIMIT);
        assert_eq!(
            events.first(),
            Some(&OscEvent::Raw(overflow.to_string().into_bytes())),
            "oldest events should have been evicted"
        );
        assert_eq!(
            events.last(),
            Some(&OscEvent::Raw(
                (CONTROL_STRING_EVENT_LIMIT + overflow - 1)
                    .to_string()
                    .into_bytes()
            )),
            "newest event should be retained"
        );

        // The unified queue is bounded by the same limit.
        assert_eq!(
            state.drain_control_strings().len(),
            CONTROL_STRING_EVENT_LIMIT
        );
    }

    #[test]
    fn sixel_queue_is_bounded_independently() {
        let mut state = ControlStringState::default();
        for i in 0..CONTROL_STRING_EVENT_LIMIT + 1 {
            state.push_dcs(DcsEvent::Sixel(SixelEvent {
                payload: i.to_string().into_bytes(),
            }));
        }
        assert_eq!(state.drain_sixel().len(), CONTROL_STRING_EVENT_LIMIT);
        assert_eq!(state.drain_dcs().len(), CONTROL_STRING_EVENT_LIMIT);
    }
}
