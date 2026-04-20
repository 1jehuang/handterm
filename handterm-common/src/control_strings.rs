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
