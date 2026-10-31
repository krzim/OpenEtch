//! `openetch`: start the OpenEtch engine. See `openetch --help` and the `engine` library docs.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use commands::Session;
use engine::cli::{self, Config, Parsed};
use engine::hw::Machine;
use engine::pin::{self, ApprovalConfig, PhysicalConfirm, PinStore};
use engine::security::{SecurityConfig, generate_token};
use engine::{AppState, api};
use machine::{MachineProfile, PollConfig};

fn main() -> ExitCode {
    let config = match cli::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(c)) => c,
        Ok(Parsed::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::SetPin { config_dir }) => return set_pin(&config_dir),
        Err(e) => {
            eprintln!("openetch: {e}");
            return ExitCode::from(2);
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,rmcp=warn".into()),
        )
        .init();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("openetch: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openetch: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `openetch set-pin`: only a person at a terminal may set the PIN.
fn set_pin(dir: &Path) -> ExitCode {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "openetch set-pin: refused: stdin is not a terminal. Run it yourself in a terminal; it cannot be scripted."
        );
        return ExitCode::from(2);
    }
    let stdin = std::io::stdin();
    match pin::set_pin_dialogue(&mut stdin.lock(), &mut std::io::stdout(), dir) {
        Ok(path) => {
            println!(
                "openetch: operator PIN saved to {} (a running engine uses it at the next approval)",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("openetch set-pin: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Load the profile and its correction table.
fn load_profile(config: &Config) -> Result<(MachineProfile, Option<corfile::CorTable>), String> {
    let text = match std::fs::read_to_string(&config.profile) {
        Ok(t) => t,
        Err(e) if config.profile_is_default => {
            tracing::warn!(path = %config.profile.display(), error = %e, "default profile not found; using the generic LMC fiber profile");
            return Ok((MachineProfile::generic_lmc_fiber(), None));
        }
        Err(e) => return Err(format!("cannot read profile {}: {e}", config.profile.display())),
    };
    let profile = MachineProfile::from_toml(&text).map_err(|e| format!("profile {}: {e}", config.profile.display()))?;
    let cor = match profile.cor_path(&config.profile) {
        Some(path) => {
            let bytes =
                std::fs::read(&path).map_err(|e| format!("cannot read correction file {}: {e}", path.display()))?;
            Some(corfile::CorTable::parse(&bytes).map_err(|e| format!("correction file {}: {e}", path.display()))?)
        }
        None => None,
    };
    Ok((profile, cor))
}

async fn run(config: Config) -> Result<(), String> {
    let (profile, cor) = load_profile(&config)?;
    tracing::info!(profile = %profile.name, field_mm = profile.field_mm, correction = cor.is_some(), "machine profile loaded");

    let machine = if config.usb {
        if !cfg!(feature = "usb") {
            return Err("--usb needs a build with USB support: cargo run -p engine --features usb -- --usb".into());
        }
        Machine::usb(profile, cor)
    } else {
        Machine::simulated(profile, cor, PollConfig::default())
    };
    let machine = Arc::new(machine);
    match machine.connect().await {
        Ok(()) => tracing::info!(simulated = !config.usb, "board ready"),
        Err(e) => tracing::warn!(error = %e, "board not ready; framing and runs will retry the connection"),
    }

    let loopback = config.listen.ip().is_loopback();
    let token = match (&config.token, loopback) {
        (Some(t), _) => Some(t.clone()),
        (None, true) => None,
        (None, false) => Some(generate_token()?),
    };
    let security = SecurityConfig::new(token.clone(), config.allow_hosts.clone());
    if let Err(e) = std::fs::create_dir_all(&config.config_dir) {
        tracing::warn!(path = %config.config_dir.display(), error = %e, "cannot create the config directory");
    }
    let approval = ApprovalConfig {
        pin: PinStore::new(&config.config_dir),
        confirm: config.confirm_input.map(PhysicalConfirm::new),
    };
    if !approval.pin.is_set() {
        println!(
            "openetch: no operator PIN is set ({}); runs cannot be approved until you run `openetch set-pin`",
            approval.pin.path().display()
        );
    }
    if let Some(c) = approval.confirm {
        println!(
            "openetch: approved runs also wait up to {} s for input {} to be released and pressed",
            c.timeout.as_secs(),
            c.input_bit
        );
    }
    let state = AppState::new(Session::new(doc::Document::default()), machine.clone(), security, approval);

    let web_dir = config.web_dir.is_dir().then_some(config.web_dir.as_path());
    if web_dir.is_none() {
        tracing::warn!(path = %config.web_dir.display(), "web UI not found (run `npm run build` in web/); serving the API only");
    }
    let app = api::router(state, web_dir);

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    let host =
        if config.listen.ip().is_unspecified() { "<this-machine>".to_owned() } else { config.listen.ip().to_string() };
    let host = if host.contains(':') && !host.starts_with('<') { format!("[{host}]") } else { host };
    let port = config.listen.port();
    match &token {
        Some(t) => println!("openetch: open http://{host}:{port}/?token={t}"),
        None => println!("openetch: open http://{host}:{port}/"),
    }
    println!(
        "openetch: MCP endpoint http://{host}:{port}/mcp{}",
        if token.is_some() { " (Authorization: Bearer <token>)" } else { "" }
    );

    let shutdown_machine = machine.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down: stopping the machine");
            if let Err(e) = shutdown_machine.abort().await {
                tracing::warn!(error = %e, "abort on shutdown failed");
            }
        })
        .await
        .map_err(|e| format!("server error: {e}"))
}
