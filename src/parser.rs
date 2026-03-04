#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    OscString,
    DcsEntry,
}

pub const MAX_PARAMS: usize = 16;

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    param_count: usize,
    current_param: u16,
    intermediate: u8,
    osc_buf: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Print(u8),
    Execute(u8),
    CsiDispatch {
        params_count: usize,
        intermediate: u8,
        final_byte: u8,
    },
    EscDispatch {
        intermediate: u8,
        final_byte: u8,
    },
    OscDispatch(Vec<u8>),
    Nop,
}

impl Parser {
    pub const fn new() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            param_count: 0,
            current_param: 0,
            intermediate: 0,
            osc_buf: Vec::new(),
        }
    }

    pub fn params(&self) -> &[u16] {
        &self.params[..self.param_count]
    }

    pub fn param(&self, idx: usize, default: u16) -> u16 {
        if idx < self.param_count && self.params[idx] != 0 {
            self.params[idx]
        } else {
            default
        }
    }

    #[inline(always)]
    pub fn advance(&mut self, byte: u8) -> Action {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::EscapeIntermediate => self.escape_intermediate(byte),
            State::CsiEntry => self.csi_entry(byte),
            State::CsiParam => self.csi_param(byte),
            State::CsiIntermediate => self.csi_intermediate(byte),
            State::OscString => self.osc_string(byte),
            State::DcsEntry => self.dcs_entry(byte),
        }
    }

    #[inline(always)]
    fn ground(&mut self, byte: u8) -> Action {
        match byte {
            0x1b => {
                self.state = State::Escape;
                Action::Nop
            }
            0x20..=0x7e | 0x80..=0xff => Action::Print(byte),
            0x00..=0x1a | 0x1c..=0x1f => Action::Execute(byte),
            _ => Action::Nop,
        }
    }

    fn escape(&mut self, byte: u8) -> Action {
        match byte {
            b'[' => {
                self.csi_reset();
                self.state = State::CsiEntry;
                Action::Nop
            }
            b']' => {
                self.state = State::OscString;
                Action::Nop
            }
            b'P' => {
                self.state = State::DcsEntry;
                Action::Nop
            }
            0x20..=0x2f => {
                self.intermediate = byte;
                self.state = State::EscapeIntermediate;
                Action::Nop
            }
            0x30..=0x7e => {
                self.state = State::Ground;
                Action::EscDispatch {
                    intermediate: 0,
                    final_byte: byte,
                }
            }
            _ => {
                self.state = State::Ground;
                Action::Nop
            }
        }
    }

    fn escape_intermediate(&mut self, byte: u8) -> Action {
        match byte {
            0x30..=0x7e => {
                let im = self.intermediate;
                self.state = State::Ground;
                Action::EscDispatch {
                    intermediate: im,
                    final_byte: byte,
                }
            }
            0x20..=0x2f => {
                self.intermediate = byte;
                Action::Nop
            }
            _ => {
                self.state = State::Ground;
                Action::Nop
            }
        }
    }

    fn csi_entry(&mut self, byte: u8) -> Action {
        match byte {
            b'0'..=b'9' => {
                self.current_param = u16::from(byte - b'0');
                self.state = State::CsiParam;
                Action::Nop
            }
            b';' => {
                self.push_param(0);
                self.state = State::CsiParam;
                Action::Nop
            }
            b'?' | b'>' | b'=' => {
                self.intermediate = byte;
                Action::Nop
            }
            0x40..=0x7e => {
                self.push_param(self.current_param);
                self.state = State::Ground;
                Action::CsiDispatch {
                    params_count: self.param_count,
                    intermediate: self.intermediate,
                    final_byte: byte,
                }
            }
            _ => {
                self.state = State::Ground;
                Action::Nop
            }
        }
    }

    fn csi_param(&mut self, byte: u8) -> Action {
        match byte {
            b'0'..=b'9' => {
                self.current_param = self
                    .current_param
                    .saturating_mul(10)
                    .saturating_add(u16::from(byte - b'0'));
                Action::Nop
            }
            b';' => {
                self.push_param(self.current_param);
                self.current_param = 0;
                Action::Nop
            }
            b':' => {
                self.push_param(self.current_param);
                self.current_param = 0;
                Action::Nop
            }
            0x20..=0x2f => {
                self.push_param(self.current_param);
                self.current_param = 0;
                self.intermediate = byte;
                self.state = State::CsiIntermediate;
                Action::Nop
            }
            0x40..=0x7e => {
                self.push_param(self.current_param);
                self.state = State::Ground;
                Action::CsiDispatch {
                    params_count: self.param_count,
                    intermediate: self.intermediate,
                    final_byte: byte,
                }
            }
            _ => {
                self.state = State::Ground;
                Action::Nop
            }
        }
    }

    fn csi_intermediate(&mut self, byte: u8) -> Action {
        match byte {
            0x20..=0x2f => {
                self.intermediate = byte;
                Action::Nop
            }
            0x40..=0x7e => {
                self.state = State::Ground;
                Action::CsiDispatch {
                    params_count: self.param_count,
                    intermediate: self.intermediate,
                    final_byte: byte,
                }
            }
            _ => {
                self.state = State::Ground;
                Action::Nop
            }
        }
    }

    fn osc_string(&mut self, byte: u8) -> Action {
        match byte {
            0x07 => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.osc_buf);
                Action::OscDispatch(data)
            }
            0x1b => {
                self.state = State::Escape;
                let data = std::mem::take(&mut self.osc_buf);
                Action::OscDispatch(data)
            }
            _ => {
                if self.osc_buf.len() < 256 {
                    self.osc_buf.push(byte);
                }
                Action::Nop
            }
        }
    }

    fn dcs_entry(&mut self, byte: u8) -> Action {
        match byte {
            0x1b => {
                self.state = State::Escape;
                Action::Nop
            }
            _ => Action::Nop,
        }
    }

    fn csi_reset(&mut self) {
        self.params = [0; MAX_PARAMS];
        self.param_count = 0;
        self.current_param = 0;
        self.intermediate = 0;
    }

    fn push_param(&mut self, value: u16) {
        if self.param_count < MAX_PARAMS {
            self.params[self.param_count] = value;
            self.param_count += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_ascii() {
        let mut p = Parser::new();
        assert_eq!(p.advance(b'A'), Action::Print(b'A'));
        assert_eq!(p.advance(b'z'), Action::Print(b'z'));
        assert_eq!(p.advance(b' '), Action::Print(b' '));
    }

    #[test]
    fn executes_control() {
        let mut p = Parser::new();
        assert_eq!(p.advance(b'\n'), Action::Execute(b'\n'));
        assert_eq!(p.advance(b'\r'), Action::Execute(b'\r'));
        assert_eq!(p.advance(b'\t'), Action::Execute(b'\t'));
    }

    #[test]
    fn parses_csi_sgr() {
        let mut p = Parser::new();

        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(p.advance(b'['), Action::Nop);
        assert_eq!(p.advance(b'1'), Action::Nop);
        assert_eq!(p.advance(b';'), Action::Nop);
        assert_eq!(p.advance(b'3'), Action::Nop);
        assert_eq!(p.advance(b'1'), Action::Nop);

        let action = p.advance(b'm');
        assert_eq!(
            action,
            Action::CsiDispatch {
                params_count: 2,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        assert_eq!(p.params(), &[1, 31]);
    }

    #[test]
    fn parses_csi_cursor_move() {
        let mut p = Parser::new();
        for &b in b"\x1b[10;20H" {
            p.advance(b);
        }
        assert_eq!(p.params(), &[10, 20]);
    }

    #[test]
    fn parses_private_mode() {
        let mut p = Parser::new();
        let mut last = Action::Nop;
        for &b in b"\x1b[?1049h" {
            last = p.advance(b);
        }
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: 1,
                intermediate: b'?',
                final_byte: b'h',
            }
        );
        assert_eq!(p.params(), &[1049]);
    }

    #[test]
    fn osc_string_terminates_on_bell() {
        let mut p = Parser::new();
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(p.advance(b']'), Action::Nop);
        assert_eq!(p.advance(b'0'), Action::Nop);
        assert_eq!(p.advance(b';'), Action::Nop);
        assert_eq!(p.advance(b'x'), Action::Nop);
        assert_eq!(p.advance(0x07), Action::OscDispatch(b"0;x".to_vec()));
        assert_eq!(p.advance(b'A'), Action::Print(b'A'));
    }

    #[test]
    fn escape_dispatch() {
        let mut p = Parser::new();
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(
            p.advance(b'M'),
            Action::EscDispatch {
                intermediate: 0,
                final_byte: b'M',
            }
        );
    }

    fn parse_throughput_mb_per_sec(input: &[u8]) -> f64 {
        let mut p = Parser::new();
        let start = std::time::Instant::now();
        for &b in input {
            std::hint::black_box(p.advance(b));
        }
        let secs = start.elapsed().as_secs_f64().max(1e-9);
        (input.len() as f64 / (1024.0 * 1024.0)) / secs
    }

    #[test]
    fn parser_throughput_exceeds_50_mb_per_sec() {
        let payload = vec![b'A'; 8 * 1024 * 1024];
        let rate = parse_throughput_mb_per_sec(&payload);
        assert!(
            rate > 50.0,
            "parser throughput too low: {rate:.0} MB/s (debug build expected >50)"
        );
    }
}
