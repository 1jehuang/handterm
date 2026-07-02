// Shared with `render.rs` and `gpu_frame.rs` test modules only.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const FISH_STARTUP_TRANSCRIPT: &[&[u8]] = &[
    b"\x1b[?u\x1b[>0q\x1b]11;?\x1b\\",
    b"\x1b[?1049h\x1bP+q696e646e\x1b\\",
    b"\x1bP+q71756572792d6f732d6e616d65\x1b\\\x1b[?1049l\x1b[0c",
];

// Outside tests, only the cpu/gpu replay benches in metrics.rs consume these.
#[cfg_attr(not(any(test, feature = "cpu", feature = "gpu")), allow(dead_code))]
pub(crate) const STARSHIP_PROMPT_PREFIX_TRANSCRIPT: &[&[u8]] =
    &[b"\x1b[J\n", b"\x1b[38;2;243;139;168m\xee\x82\xb6"];

#[cfg_attr(not(any(test, feature = "cpu", feature = "gpu")), allow(dead_code))]
pub(crate) const STARSHIP_PROMPT_FINAL_CHUNK: &[u8] = &[
    0x1b, 0x5b, 0x34, 0x38, 0x3b, 0x32, 0x3b, 0x32, 0x34, 0x33, 0x3b, 0x31, 0x33, 0x39, 0x3b, 0x31,
    0x36, 0x38, 0x3b, 0x33, 0x38, 0x3b, 0x32, 0x3b, 0x31, 0x37, 0x3b, 0x31, 0x37, 0x3b, 0x32, 0x37,
    0x6d, 0xf3, 0xb0, 0xa3, 0x87, 0x20, 0x6a, 0x65, 0x72, 0x65, 0x6d, 0x79, 0x1b, 0x5b, 0x30, 0x6d,
    0x20,
];

#[cfg_attr(not(any(test, feature = "cpu", feature = "gpu")), allow(dead_code))]
pub(crate) const STARSHIP_PROMPT_TRANSCRIPT: &[&[u8]] = &[
    STARSHIP_PROMPT_PREFIX_TRANSCRIPT[0],
    STARSHIP_PROMPT_PREFIX_TRANSCRIPT[1],
    STARSHIP_PROMPT_FINAL_CHUNK,
];

// Shared with the `render.rs` test module only.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const TUI_HELP_OVERLAY_TRANSCRIPT: &[&[u8]] = &[
    b"\x1b[?1049h",
    b"jcode boot\r\nloading modules\r\nready\r\n",
    b"\x1b[2J\x1b[H",
    b"/help\r\n",
    b"\x1b[38;5;39mcommands\x1b[0m\r\nopen close save\r\n",
];

#[cfg_attr(not(any(test, feature = "cpu", feature = "gpu")), allow(dead_code))]
pub(crate) const TUI_HELP_WITH_IMAGE_TRANSCRIPT: &[&[u8]] = &[
    b"\x1b[?1049h",
    b"jcode boot\r\nloading modules\r\nready\r\n",
    b"\x1b_Ga=T,i=5,f=32,s=1,v=1,c=2,r=1;/wAA/w==\x1b\\",
    b"\x1b[2J\x1b[H",
    b"/help\r\n",
    b"\x1b[38;5;39mcommands\x1b[0m\r\nopen close save\r\n",
];

#[cfg_attr(not(any(test, feature = "cpu", feature = "gpu")), allow(dead_code))]
pub(crate) const EMOJI_AND_SHADE_TRANSCRIPT: &[&[u8]] = &[
    "status ".as_bytes(),
    "❤️".as_bytes(),
    " 👨‍💻\r\n".as_bytes(),
    "░░░░░░░░░░".as_bytes(),
];

// Only the cpu-feature render benchmark (metrics.rs) drives these workloads.
#[cfg(feature = "cpu")]
pub(crate) const PROMPT_PREFIX: &[u8] = b"\x1b[38;5;10m>\x1b[0m ";
#[cfg(feature = "cpu")]
pub(crate) const TYPING_WORKLOAD: &[u8] = b"echo hello world from handterm";
