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
    OscEscape,
    DcsString,
    DcsEscape,
    ApcString,
    ApcEscape,
}

pub const MAX_PARAMS: usize = 16;

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    param_count: usize,
    current_param: u16,
    intermediate: u8,
    osc_buf: Vec<u8>,
    dcs_buf: Vec<u8>,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Print(u8),
    Execute(u8),
    CsiDispatch {
        params_count: u8,
        intermediate: u8,
        final_byte: u8,
    },
    EscDispatch {
        intermediate: u8,
        final_byte: u8,
    },
    OscDispatch(Box<Vec<u8>>),
    DcsDispatch(Box<Vec<u8>>),
    ApcDispatch(Box<Vec<u8>>),
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
            dcs_buf: Vec::new(),
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
    pub fn is_ground(&self) -> bool {
        matches!(self.state, State::Ground)
    }

    #[inline(always)]
    pub fn advance(&mut self, byte: u8) -> Action {
        if self.state as u8 == 0 {
            if byte >= 0x20 {
                return Action::Print(byte);
            }
            if byte == 0x1b {
                self.state = State::Escape;
                return Action::Nop;
            }
            return Action::Execute(byte);
        }
        self.advance_slow(byte)
    }

    #[inline(never)]
    fn advance_slow(&mut self, byte: u8) -> Action {
        match self.state {
            State::Ground => unreachable!(),
            State::Escape => self.escape(byte),
            State::EscapeIntermediate => self.escape_intermediate(byte),
            State::CsiEntry => self.csi_entry(byte),
            State::CsiParam => self.csi_param(byte),
            State::CsiIntermediate => self.csi_intermediate(byte),
            State::OscString => self.osc_string(byte),
            State::OscEscape => self.osc_escape(byte),
            State::DcsString => self.dcs_string(byte),
            State::DcsEscape => self.dcs_escape(byte),
            State::ApcString => self.apc_string(byte),
            State::ApcEscape => self.apc_escape(byte),
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
                self.dcs_buf.clear();
                self.state = State::DcsString;
                Action::Nop
            }
            b'_' => {
                self.osc_buf.clear();
                self.state = State::ApcString;
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
            b'?' | b'>' | b'=' | b'<' => {
                self.intermediate = byte;
                Action::Nop
            }
            0x40..=0x7e => {
                self.push_param(self.current_param);
                self.state = State::Ground;
                Action::CsiDispatch {
                    params_count: self.param_count as u8,
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
                    params_count: self.param_count as u8,
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
                    params_count: self.param_count as u8,
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
                Action::OscDispatch(Box::new(data))
            }
            0x1b => {
                self.state = State::OscEscape;
                Action::Nop
            }
            _ => {
                if self.osc_buf.len() < 4096 {
                    self.osc_buf.push(byte);
                }
                Action::Nop
            }
        }
    }

    fn osc_escape(&mut self, byte: u8) -> Action {
        match byte {
            b'\\' => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.osc_buf);
                Action::OscDispatch(Box::new(data))
            }
            other => {
                self.state = State::OscString;
                if self.osc_buf.len() + 2 <= 4096 {
                    self.osc_buf.push(0x1b);
                    self.osc_buf.push(other);
                }
                Action::Nop
            }
        }
    }

    fn dcs_string(&mut self, byte: u8) -> Action {
        match byte {
            0x1b => {
                self.state = State::DcsEscape;
                Action::Nop
            }
            0x07 => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.dcs_buf);
                Action::DcsDispatch(Box::new(data))
            }
            _ => {
                if self.dcs_buf.len() < 1024 * 1024 {
                    self.dcs_buf.push(byte);
                }
                Action::Nop
            }
        }
    }

    fn dcs_escape(&mut self, byte: u8) -> Action {
        match byte {
            b'\\' => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.dcs_buf);
                Action::DcsDispatch(Box::new(data))
            }
            other => {
                self.state = State::DcsString;
                if self.dcs_buf.len() + 2 <= 1024 * 1024 {
                    self.dcs_buf.push(0x1b);
                    self.dcs_buf.push(other);
                }
                Action::Nop
            }
        }
    }

    fn apc_string(&mut self, byte: u8) -> Action {
        match byte {
            0x1b => {
                self.state = State::ApcEscape;
                Action::Nop
            }
            0x07 => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.osc_buf);
                Action::ApcDispatch(Box::new(data))
            }
            _ => {
                if self.osc_buf.len() < 1024 * 1024 {
                    self.osc_buf.push(byte);
                }
                Action::Nop
            }
        }
    }

    fn apc_escape(&mut self, byte: u8) -> Action {
        match byte {
            b'\\' => {
                self.state = State::Ground;
                let data = std::mem::take(&mut self.osc_buf);
                Action::ApcDispatch(Box::new(data))
            }
            other => {
                self.state = State::ApcString;
                if self.osc_buf.len() + 2 <= 1024 * 1024 {
                    self.osc_buf.push(0x1b);
                    self.osc_buf.push(other);
                }
                Action::Nop
            }
        }
    }

    fn csi_reset(&mut self) {
        // No need to clear the `params` array: every reader is bounded by
        // `param_count`, and `push_param` always writes `params[param_count]`
        // before advancing the count, so stale slots are never observed.
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
        assert_eq!(
            p.advance(0x07),
            Action::OscDispatch(Box::new(b"0;x".to_vec()))
        );
        assert_eq!(p.advance(b'A'), Action::Print(b'A'));
    }

    #[test]
    fn osc_dispatches_payload_on_st() {
        let mut p = Parser::new();
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(p.advance(b']'), Action::Nop);
        assert_eq!(p.advance(b'0'), Action::Nop);
        assert_eq!(p.advance(b';'), Action::Nop);
        assert_eq!(p.advance(b'x'), Action::Nop);
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(
            p.advance(b'\\'),
            Action::OscDispatch(Box::new(b"0;x".to_vec()))
        );
    }

    #[test]
    fn dcs_dispatches_payload() {
        let mut p = Parser::new();
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(p.advance(b'P'), Action::Nop);
        assert_eq!(p.advance(b'+'), Action::Nop);
        assert_eq!(p.advance(b'q'), Action::Nop);
        assert_eq!(p.advance(b'1'), Action::Nop);
        assert_eq!(p.advance(b'2'), Action::Nop);
        assert_eq!(p.advance(0x1b), Action::Nop);
        assert_eq!(
            p.advance(b'\\'),
            Action::DcsDispatch(Box::new(b"+q12".to_vec()))
        );
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

    /// The returned `Action` is produced for every parsed byte on the hot path,
    /// so it must stay small enough to be returned in registers (no hidden
    /// sret/return-slot copy). Keep it at or below two machine words.
    #[test]
    fn action_is_register_sized() {
        assert!(
            std::mem::size_of::<Action>() <= 16,
            "Action grew to {} bytes; the hot parser path depends on it fitting in registers",
            std::mem::size_of::<Action>()
        );
    }

    /// `csi_reset` no longer zeroes the whole params array; it relies on
    /// `param_count` bounding every reader. Verify a shorter CSI sequence after
    /// a longer one never exposes stale parameters from the previous sequence.
    #[test]
    fn csi_params_do_not_leak_between_sequences() {
        let mut p = Parser::new();
        for &b in b"\x1b[11;22;33;44m" {
            p.advance(b);
        }
        assert_eq!(p.params(), &[11, 22, 33, 44]);

        // A subsequent shorter sequence must report only its own params.
        let mut last = Action::Nop;
        for &b in b"\x1b[9m" {
            last = p.advance(b);
        }
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: 1,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        assert_eq!(p.params(), &[9]);
        assert_eq!(p.param(0, 0), 9);
        // Index 1 is out of range for this sequence and must fall back to default,
        // not the stale `22` from the previous sequence.
        assert_eq!(p.param(1, 7), 7);
    }

    /// OSC/DCS/APC dispatch payloads are now boxed; confirm the payload still
    /// round-trips byte-identically and the boxed value derefs as a slice.
    #[test]
    fn boxed_dispatch_payload_roundtrips() {
        let mut p = Parser::new();
        for &b in b"\x1b]2;hello world\x07" {
            let action = p.advance(b);
            if let Action::OscDispatch(data) = action {
                assert_eq!(&data[..], b"2;hello world");
                return;
            }
        }
        panic!("expected an OscDispatch action");
    }

    /// Feed a byte slice and return the last action plus all dispatches seen.
    fn run(p: &mut Parser, bytes: &[u8]) -> Action {
        let mut last = Action::Nop;
        for &b in bytes {
            last = p.advance(b);
        }
        last
    }

    #[test]
    fn params_beyond_max_are_dropped_not_wrapped() {
        let mut p = Parser::new();
        let mut seq = b"\x1b[".to_vec();
        for i in 1..=24 {
            if i > 1 {
                seq.push(b';');
            }
            seq.extend_from_slice(i.to_string().as_bytes());
        }
        seq.push(b'm');

        let last = run(&mut p, &seq);
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: MAX_PARAMS as u8,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        // The first MAX_PARAMS values are kept verbatim; the excess is dropped.
        let expected: Vec<u16> = (1..=MAX_PARAMS as u16).collect();
        assert_eq!(p.params(), expected.as_slice());
    }

    #[test]
    fn huge_numeric_param_saturates_at_u16_max() {
        let mut p = Parser::new();
        run(&mut p, b"\x1b[99999999999999999999H");
        assert_eq!(p.params(), &[u16::MAX]);
    }

    #[test]
    fn colon_subparams_split_like_semicolons() {
        // SGR 38:2:255:0:0 (colon-separated direct color) must not corrupt
        // neighboring params.
        let mut p = Parser::new();
        let last = run(&mut p, b"\x1b[38:2:255:0:0m");
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: 5,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        assert_eq!(p.params(), &[38, 2, 255, 0, 0]);
    }

    /// A C0 control interleaved inside a CSI sequence aborts the sequence and
    /// returns the parser to ground, so the following text prints normally.
    #[test]
    fn c0_control_inside_csi_aborts_sequence() {
        let mut p = Parser::new();
        assert_eq!(run(&mut p, b"\x1b[12"), Action::Nop);
        assert_eq!(p.advance(0x0e), Action::Nop); // SO aborts the CSI
        assert!(p.is_ground());
        assert_eq!(p.advance(b'A'), Action::Print(b'A'));
    }

    /// An unterminated OSC swallows following text until a real terminator,
    /// then the parser recovers and handles the next sequence normally.
    #[test]
    fn unterminated_osc_recovers_on_terminator() {
        let mut p = Parser::new();
        assert_eq!(run(&mut p, b"\x1b]0;titleplain text after"), Action::Nop);
        assert_eq!(
            p.advance(0x07),
            Action::OscDispatch(Box::new(b"0;titleplain text after".to_vec()))
        );
        // A complete follow-up sequence parses cleanly from ground.
        let last = run(&mut p, b"\x1b[3m");
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: 1,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        assert_eq!(p.params(), &[3]);
    }

    #[test]
    fn embedded_escape_in_osc_payload_is_preserved() {
        // ESC followed by anything other than `\` is data, not a terminator.
        let mut p = Parser::new();
        assert_eq!(run(&mut p, b"\x1b]0;a\x1bXb"), Action::Nop);
        assert_eq!(
            p.advance(0x07),
            Action::OscDispatch(Box::new(b"0;a\x1bXb".to_vec()))
        );
    }

    #[test]
    fn c0_controls_inside_osc_are_buffered_as_data() {
        let mut p = Parser::new();
        run(&mut p, b"\x1b]0;a\nb");
        assert_eq!(
            p.advance(0x07),
            Action::OscDispatch(Box::new(b"0;a\nb".to_vec()))
        );
    }

    #[test]
    fn osc_payload_is_capped_at_4096_bytes() {
        let mut p = Parser::new();
        run(&mut p, b"\x1b]");
        for _ in 0..5000 {
            assert_eq!(p.advance(b'x'), Action::Nop);
        }
        match p.advance(0x07) {
            Action::OscDispatch(data) => {
                assert_eq!(data.len(), 4096, "OSC payload should be truncated");
            }
            other => panic!("expected OscDispatch, got {other:?}"),
        }
    }

    #[test]
    fn apc_dispatches_on_st_and_bel() {
        let mut p = Parser::new();
        assert_eq!(run(&mut p, b"\x1b_Gf=32\x1b"), Action::Nop);
        assert_eq!(
            p.advance(b'\\'),
            Action::ApcDispatch(Box::new(b"Gf=32".to_vec()))
        );

        // BEL also terminates an APC string.
        assert_eq!(run(&mut p, b"\x1b_Ga=d"), Action::Nop);
        assert_eq!(
            p.advance(0x07),
            Action::ApcDispatch(Box::new(b"Ga=d".to_vec()))
        );
    }

    #[test]
    fn embedded_escape_in_apc_and_dcs_payloads_is_preserved() {
        let mut p = Parser::new();
        run(&mut p, b"\x1b_a\x1bZb\x1b");
        assert_eq!(
            p.advance(b'\\'),
            Action::ApcDispatch(Box::new(b"a\x1bZb".to_vec()))
        );

        run(&mut p, b"\x1bPc\x1bZd\x1b");
        assert_eq!(
            p.advance(b'\\'),
            Action::DcsDispatch(Box::new(b"c\x1bZd".to_vec()))
        );
    }

    /// SOS (ESC X) and PM (ESC ^) have no string-consuming state in this
    /// parser: the introducer dispatches as a plain escape and the payload
    /// prints. Pin that down so a future change is a conscious decision.
    #[test]
    fn sos_and_pm_introducers_dispatch_as_escapes() {
        for intro in [b'X', b'^'] {
            let mut p = Parser::new();
            assert_eq!(p.advance(0x1b), Action::Nop);
            assert_eq!(
                p.advance(intro),
                Action::EscDispatch {
                    intermediate: 0,
                    final_byte: intro,
                }
            );
            assert!(p.is_ground());
        }
    }

    #[test]
    fn utf8_multibyte_garbage_prints_bytewise_from_ground() {
        // Raw UTF-8 continuation bytes and truncated multibyte prefixes are
        // the terminal layer's problem; the parser must pass them through.
        let mut p = Parser::new();
        for &b in "é€\u{10348}".as_bytes() {
            assert_eq!(p.advance(b), Action::Print(b));
        }
        // Lone continuation byte and overlong-encoding prefix also print.
        assert_eq!(p.advance(0xbf), Action::Print(0xbf));
        assert_eq!(p.advance(0xc0), Action::Print(0xc0));
    }

    /// Deterministic pseudo-random byte storm: the parser must never panic
    /// and its invariants (bounded params, bounded payloads) must hold on
    /// every single byte.
    #[test]
    fn never_panics_on_pseudo_random_bytes() {
        let mut rng_state: u64 = 0x1531_5731_dead_beef;
        let mut next = move || {
            // xorshift64: deterministic, no dependencies.
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };

        let mut p = Parser::new();
        for _ in 0..4000 {
            let len = (next() % 64) as usize;
            for _ in 0..len {
                let byte = (next() >> 32) as u8;
                let action = p.advance(byte);
                assert!(p.params().len() <= MAX_PARAMS);
                match action {
                    Action::CsiDispatch { params_count, .. } => {
                        assert!(usize::from(params_count) <= MAX_PARAMS);
                    }
                    Action::OscDispatch(data) => {
                        assert!(data.len() <= 4096, "OSC payload exceeded cap");
                    }
                    Action::DcsDispatch(data) | Action::ApcDispatch(data) => {
                        assert!(data.len() <= 1024 * 1024, "string payload exceeded cap");
                    }
                    _ => {}
                }
            }
        }

        // After any garbage the parser must still parse a clean sequence.
        run(&mut p, b"\x07\x1b\\"); // best-effort flush to ground
        run(&mut p, b"\x1b[0m");
        let last = run(&mut p, b"\x1b[1;31m");
        assert_eq!(
            last,
            Action::CsiDispatch {
                params_count: 2,
                intermediate: 0,
                final_byte: b'm',
            }
        );
        assert_eq!(p.params(), &[1, 31]);
    }
}
