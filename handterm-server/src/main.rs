use anyhow::Result;
use handterm::config::AppConfig;

fn main() -> Result<()> {
    let config = AppConfig::load(None)?;
    handterm::daemon::run_server_only(None, &config)
}
