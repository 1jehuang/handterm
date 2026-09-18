use handterm_common::terminal::Terminal;

const QUERIES: &[u8] = b"\x1b]10;?\x07\x1b]11;?\x1b\\";
const ORIGINAL: &[u8] = b"\x1b]10;rgb:cd/d6/f4\x1b\\\x1b]11;rgb:00/00/00\x1b\\";
const LIGHT: &[u8] = b"\x1b]10;rgb:01/23/45\x1b\\\x1b]11;rgb:fa/fb/ff\x1b\\";
const DARK: &[u8] = b"\x1b]10;rgb:ef/cd/ab\x1b\\\x1b]11;rgb:00/08/10\x1b\\";

fn assert_queries(terminal: &mut Terminal, expected: &[u8]) {
    terminal.process(QUERIES);
    assert_eq!(terminal.drain_responses().as_deref(), Some(expected));
    assert_eq!(terminal.drain_responses(), None);
}

#[test]
fn constructor_defaults_are_unchanged() {
    for mut terminal in [
        Terminal::new(80, 24),
        Terminal::new_with_scrollback(80, 24, 7),
    ] {
        assert_queries(&mut terminal, ORIGINAL);
        terminal.process(b"\x1bc");
        assert_queries(&mut terminal, ORIGINAL);
    }
}

#[test]
fn theme_changes_report_actual_light_and_dark_colors() {
    let mut terminal = Terminal::new(80, 24);
    terminal.set_default_colors([0x01, 0x23, 0x45], [0xfa, 0xfb, 0xff]);
    assert_queries(&mut terminal, LIGHT);
    terminal.set_default_colors([0xef, 0xcd, 0xab], [0x00, 0x08, 0x10]);
    // Explicit SGR colors and attribute resets must not change theme defaults.
    terminal.process(b"\x1b[38;2;255;0;0;48;2;0;255;0mX\x1b[0m");
    assert_queries(&mut terminal, DARK);
}

#[test]
fn fragmented_queries_support_bel_and_st_terminators() {
    for query in [
        b"\x1b]10;?\x07".as_slice(),
        b"\x1b]10;?\x1b\\".as_slice(),
        b"\x1b]11;?\x07".as_slice(),
        b"\x1b]11;?\x1b\\".as_slice(),
    ] {
        for split in 0..query.len() {
            let mut terminal = Terminal::new(80, 24);
            terminal.set_default_colors([0x01, 0x23, 0x45], [0xfa, 0xfb, 0xff]);
            terminal.process(&query[..split]);
            assert_eq!(terminal.drain_responses(), None);
            terminal.process(&query[split..]);
            let expected = if query[3] == b'0' {
                b"\x1b]10;rgb:01/23/45\x1b\\".as_slice()
            } else {
                b"\x1b]11;rgb:fa/fb/ff\x1b\\".as_slice()
            };
            assert_eq!(terminal.drain_responses().as_deref(), Some(expected));
        }
    }
    let mut terminal = Terminal::new(80, 24);
    terminal.set_default_colors([0xef, 0xcd, 0xab], [0x00, 0x08, 0x10]);
    for byte in QUERIES {
        terminal.process(&[*byte]);
    }
    assert_eq!(terminal.drain_responses().as_deref(), Some(DARK));
}

#[test]
fn reset_and_screen_switches_preserve_embedder_defaults() {
    for (foreground, background, expected) in [
        ([0x01, 0x23, 0x45], [0xfa, 0xfb, 0xff], LIGHT),
        ([0xef, 0xcd, 0xab], [0x00, 0x08, 0x10], DARK),
    ] {
        let mut terminal = Terminal::new_with_scrollback(80, 24, 7);
        terminal.set_default_colors(foreground, background);
        terminal.process(b"\x1b[?1049h");
        assert_queries(&mut terminal, expected);
        terminal.process(b"\x1b[?1049l");
        assert_queries(&mut terminal, expected);
        terminal.process(b"text\x1b[31m\x1bc");
        assert_eq!(terminal.grid.cell_char(0, 0), ' ');
        assert_eq!(terminal.scrollback_limit(), 7);
        assert_queries(&mut terminal, expected);
        terminal.process(b"\x1b[?1049h\x1bc");
        assert_queries(&mut terminal, expected);
    }
}
