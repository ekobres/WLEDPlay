//! Live WLEDPlay log level. `RUST_LOG` still wins at process start.

use std::sync::OnceLock;
use tracing_subscriber::EnvFilter;

type ReloadFn = Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>;

static RELOAD: OnceLock<ReloadFn> = OnceLock::new();

pub fn filter_for(level: &str) -> EnvFilter {
    EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"))
}

fn flatten_reload(result: Result<(), tracing_subscriber::reload::Error>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn dispatch(handle: Option<&ReloadFn>, level: &str) -> Result<(), String> {
    let filter = EnvFilter::try_new(level).map_err(|err| err.to_string())?;
    match handle {
        Some(reload) => reload(filter),
        None => Ok(()),
    }
}

pub fn install(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| filter_for(level));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_filter_reloading();
    let handle = builder.reload_handle();
    builder.try_init().ok();
    let _ = RELOAD.set(Box::new(move |filter| {
        flatten_reload(handle.reload(filter))
    }));
}

pub fn reload(level: &str) -> Result<(), String> {
    dispatch(RELOAD.get(), level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_falls_back_and_reload_reports_both_outcomes() {
        assert!(filter_for("debug").to_string().contains("debug"));
        assert!(filter_for("!!!").to_string().contains("info"));
        assert!(dispatch(None, "info").is_ok());
        assert!(dispatch(None, "!!!").is_err());
        let ok: ReloadFn = Box::new(|_| Ok(()));
        assert!(dispatch(Some(&ok), "debug").is_ok());
        let fail: ReloadFn = Box::new(|_| Err("down".into()));
        assert_eq!(dispatch(Some(&fail), "info").unwrap_err(), "down");
        assert!(flatten_reload(Ok(())).is_ok());
        let builder = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info"))
            .with_ansi(false)
            .with_filter_reloading();
        let handle = builder.reload_handle();
        drop(builder);
        let err = flatten_reload(handle.reload(EnvFilter::new("debug"))).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn install_stores_a_handle_that_reload_can_call() {
        install("info");
        let _ = reload("debug");
        install("info");
        let _ = reload("!!!");
    }
}
