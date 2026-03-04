use crate::grid::Grid;
use crate::pty::PtyChild;
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BenchResult {
    pub spawn_us: u128,
    pub shell_ready_us: u128,
    pub ascii_grid_mb_per_sec: f64,
    pub grid_alloc_us: u128,
}

pub fn run_quick_bench(columns: u16, rows: u16) -> Result<BenchResult> {
    let t0 = Instant::now();
    let pty = PtyChild::spawn_default_shell(columns, rows)?;
    let spawn_us = t0.elapsed().as_micros();

    let marker = format!("handterm-ready-{}", std::process::id());
    let cmd = format!("printf '{}\\n'\\n", marker);
    let ready_start = Instant::now();
    pty.write_all(cmd.as_bytes())?;

    let mut read_buf = vec![0_u8; 16 * 1024];
    let mut got = String::new();
    let deadline = Instant::now() + Duration::from_secs(2);

    while Instant::now() < deadline {
        let n = pty.try_read(&mut read_buf)?;
        if n == 0 {
            continue;
        }
        got.push_str(&String::from_utf8_lossy(&read_buf[..n]));
        if got.contains(&marker) {
            break;
        }
    }

    if !got.contains(&marker) {
        bail!("shell ready marker was not observed within timeout");
    }

    let shell_ready_us = ready_start.elapsed().as_micros();

    let grid_start = Instant::now();
    let mut grid = Grid::new(columns, rows, [0xcd, 0xd6, 0xf4], [0x00, 0x00, 0x00]);
    let grid_alloc_us = grid_start.elapsed().as_micros();

    let payload = vec![b'x'; 8 * 1024 * 1024];
    let parse_start = Instant::now();
    grid.write_bytes(&payload);
    let secs = parse_start.elapsed().as_secs_f64().max(1e-9);
    let ascii_grid_mb_per_sec = (payload.len() as f64 / (1024.0 * 1024.0)) / secs;

    Ok(BenchResult {
        spawn_us,
        shell_ready_us,
        ascii_grid_mb_per_sec,
        grid_alloc_us,
    })
}

#[cfg(test)]
mod tests {
    use super::run_quick_bench;

    #[test]
    fn quick_bench_runs_and_produces_metrics() {
        let out = run_quick_bench(80, 24).expect("bench should run");
        assert!(out.spawn_us < 2_000_000);
        assert!(out.shell_ready_us < 2_000_000);
        assert!(out.ascii_grid_mb_per_sec > 1.0);
    }
}
