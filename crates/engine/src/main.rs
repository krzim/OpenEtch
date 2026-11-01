//! `openetch`: start the OpenEtch engine. See `openetch --help` and the `engine` library docs.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use commands::Session;
use engine::cli::{self, Config, Parsed};
use engine::devices::{self, DeviceRegistry};
use engine::hw::Machine;
use engine::pin::{self, ApprovalConfig, PhysicalConfirm, PinStore};
use engine::projects::ProjectDir;
use engine::security::{SecurityConfig, generate_token};
use engine::{AppState, api};
use machine::PollConfig;

fn main() -> ExitCode {
    let config = match cli::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(c)) => c,
        Ok(Parsed::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::SetPin { config_dir, reset }) => return set_pin(&config_dir, reset),
        Ok(Parsed::Devices(cmd)) => {
            return match engine::devices::devices_command(&cmd) {
                Ok(report) => {
                    print!("{report}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("openetch devices: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(Parsed::ProfileFromLightBurn(opts)) => {
            return match engine::lightburn::profile_from_lightburn_command(&opts) {
                Ok(report) => {
                    print!("{report}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("openetch profile from-lightburn: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Err(e) => {
            eprintln!("openetch: {e}");
            return ExitCode::from(2);
        }
    };
    {
        use tracing_subscriber::prelude::*;
        let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,rmcp=warn".into());
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(filter))
            // The diagnostics log (bug reports, debug log toggle).
            .with(engine::diagnostics::RingLayer.with_filter(engine::diagnostics::ring_filter()))
            .init();
    }
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

/// `openetch set-pin [--reset]`: only a person at a terminal may set, change or delete the PIN.
fn set_pin(dir: &Path, reset: bool) -> ExitCode {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "openetch set-pin: refused: stdin is not a terminal. Run it yourself in a terminal; it cannot be scripted."
        );
        return ExitCode::from(2);
    }
    let stdin = std::io::stdin();
    let (mut input, mut out) = (stdin.lock(), std::io::stdout());
    let result = if reset {
        pin::reset_pin_dialogue(&mut input, &mut out, dir).map(|outcome| match outcome {
            pin::ResetOutcome::Deleted(path) => format!(
                "operator PIN deleted ({}). Approvals stay disabled until you set a new PIN with `openetch set-pin` \
                 and restart the engine.",
                path.display()
            ),
            pin::ResetOutcome::NothingToDelete(path) => format!("no operator PIN was set ({})", path.display()),
        })
    } else {
        pin::set_pin_dialogue(&mut input, &mut out, dir).map(|path| {
            format!(
                "operator PIN saved to {}. Restart the engine to use it: a running engine refuses approvals once \
                 the PIN file changes.",
                path.display()
            )
        })
    };
    match result {
        Ok(msg) => {
            println!("openetch: {msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("openetch set-pin: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--sim-pin` is refused unless the engine drives the simulator (and, in a release build, unless
/// `OPENETCH_ALLOW_SIM_PIN=1`).
fn check_sim_pin(config: &Config, simulated: bool) -> Result<(), String> {
    if config.sim_pin.is_none() {
        return Ok(());
    }
    let env = std::env::var(pin::ALLOW_SIM_PIN_ENV).ok();
    pin::check_sim_pin_allowed(simulated && !config.usb, cfg!(debug_assertions), env.as_deref())
}

async fn run(config: Config) -> Result<(), String> {
    check_sim_pin(&config, !config.usb)?;
    let (builtin, builtin_cor) = devices::load_profile(&config.profile, config.profile_is_default)?;
    tracing::info!(profile = %builtin.name, field_mm = builtin.field_mm, correction = builtin_cor.is_some(), "machine profile loaded");

    // Devices: the registry in <config-dir>/devices (a first run imports LightBurn's devices on this computer), and
    // the active one, or --device for this run.
    let prefs_paths = match &config.lightburn_prefs {
        Some(p) => vec![p.clone()],
        None => engine::lightburn::default_prefs_paths(),
    };
    let opened = DeviceRegistry::open(
        config.config_dir.join(devices::DEVICES_DIR),
        builtin.clone(),
        builtin_cor.clone(),
        Some(&config.profile),
        || devices::first_run_prefs(&prefs_paths),
    );
    let mut registry = match opened {
        Ok((r, notes)) => {
            for n in notes {
                println!("openetch: {n}");
            }
            r
        }
        Err(e) => {
            println!("openetch: devices: {e}; using the built-in device only");
            DeviceRegistry::in_memory(builtin.clone(), builtin_cor.clone())
        }
    };
    if let Some(query) = &config.device {
        registry.override_active(query).map_err(|e| format!("--device: {e}"))?;
    }
    let active = registry.active().clone();
    let (profile, cor) = match active.profile.clone().filter(|_| active.controllable) {
        Some(p) => {
            let (cor, warning) = registry.correction_for(&active);
            if let Some(w) = warning {
                println!("openetch: device \"{}\": {w}", active.display_name);
            }
            (p, cor)
        }
        None => (builtin, builtin_cor),
    };
    match active.not_controllable_reason() {
        None => println!("openetch: device \"{}\" ({})", active.display_name, active.id),
        Some(reason) => println!("openetch: {reason}; framing and runs are off until a JCZ galvo is made active"),
    }

    let machine = if config.usb {
        if !cfg!(feature = "usb") {
            return Err("--usb needs a build with USB support: cargo run -p engine --features usb -- --usb".into());
        }
        Machine::usb(profile, cor)
    } else {
        Machine::simulated(profile, cor, PollConfig::default())
    };
    let machine = Arc::new(machine);
    // Again against the machine actually built: never a simulator PIN on a USB link.
    check_sim_pin(&config, machine.kind() == engine::hw::LinkKind::Simulated)?;
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
    if let Err(e) = pin::create_config_dir(&config.config_dir) {
        tracing::warn!(path = %config.config_dir.display(), error = %e, "cannot set up the config directory");
    }
    let pin_store = match &config.sim_pin {
        Some(p) => PinStore::simulator(&config.config_dir, p)?,
        None => PinStore::new(&config.config_dir),
    };
    let approval = ApprovalConfig { pin: pin_store, confirm: config.confirm_input.map(PhysicalConfirm::new) };
    if approval.pin.is_sim() {
        let banner = "SIMULATOR TEST PIN ACTIVE (--sim-pin): runs on this engine are approved with a PIN given on the \
                      command line, held in memory only. The engine drives the simulator; nothing reaches a laser. \
                      Never use --sim-pin on an engine that drives a laser.";
        tracing::warn!("{banner}");
        eprintln!("openetch: WARNING: {banner}");
    } else if !approval.pin.is_set() {
        println!(
            "openetch: no operator PIN is set ({}); runs cannot be approved until you run `openetch set-pin` and \
             restart the engine",
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
    // The document starts laid out for the active device (AppState::new also fills a field in).
    let document = engine::state::new_document_for(&active, machine.profile().field_mm);
    let projects = ProjectDir::new(&config.projects_dir);
    tracing::info!(path = %config.projects_dir.display(), "projects directory");
    let state = AppState::new(Session::new(document), machine.clone(), security, approval, projects);
    state.install_devices(registry);
    state.job.open(config.config_dir.join(engine::job::COUNTERS_FILE));
    // Libraries, preferences, job log, camera calibrations (module `libs`).
    for note in state.libs.open(&config.config_dir) {
        println!("openetch: {note}");
    }
    engine::libs::select_device_library(&state);
    engine::libs::apply_startup_defaults(&state);
    engine::libs::spawn_io_watch(state.clone());
    for note in state.files.open(&config.config_dir) {
        println!("openetch: {note}");
    }
    engine::userfiles::spawn_autosave(state.clone());
    let lightburn = engine::lightburn::LightBurnConfig {
        prefs_paths: match &config.lightburn_prefs {
            Some(p) => vec![p.clone()],
            None => engine::lightburn::default_prefs_paths(),
        },
        device: config.lightburn_device.clone(),
        store_dir: Some(config.config_dir.clone()),
    };
    if let Some(p) = config.lightburn_prefs.as_ref().filter(|p| !p.is_file()) {
        tracing::warn!(path = %p.display(), "--lightburn-prefs: no such file; LightBurn devices are unavailable until it exists");
    }
    let summary = state.lightburn.configure(lightburn);
    println!("openetch: {summary}");
    if let Some(name) = &config.lightburn_device
        && state.lightburn.matched().is_none()
    {
        tracing::warn!(device = %name, "--lightburn-device: no such LightBurn device");
    }

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
