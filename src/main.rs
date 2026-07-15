#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod deviceloss;
mod log;
mod model;
mod render;
mod shell;
mod status;
mod tracker;

use anyhow::{Context, Result};
use std::{env, process};

fn main() {
    if let Err(error) = real_main() {
        shell::force_restore_taskbars();
        eprintln!("YumeDock failed: {error:#}");
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                &windows::core::HSTRING::from(format!(
                    "YumeDock could not start:\n\n{error:#}\n\nThe Windows taskbar was restored."
                )),
                windows::core::w!("YumeDock"),
                Default::default(),
            );
        }
        process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.get(1).is_some_and(|a| a == "--watchdog") {
        let pid = args
            .get(2)
            .context("watchdog parent PID missing")?
            .parse()
            .context("invalid watchdog parent PID")?;
        let appbar_state = args
            .get(3)
            .context("watchdog taskbar state missing")?
            .parse()
            .context("invalid watchdog taskbar state")?;
        return shell::run_watchdog(pid, appbar_state);
    }
    let _single_instance = unsafe {
        use windows::Win32::{
            Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
            System::Threading::CreateMutexW,
        };
        let handle = CreateMutexW(
            None,
            true,
            windows::core::w!("Local\\YumeDock.SingleInstance"),
        )?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        handle
    };
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
        .ok()?;
    }
    let safe_mode = args.iter().any(|a| a == "--safe-mode");
    let config = config::ConfigV1::load().unwrap_or_else(|error| {
        eprintln!("using default config: {error:#}");
        config::ConfigV1::default()
    });
    if config.behavior.replace_taskbar && !safe_mode {
        shell::spawn_watchdog()?;
    }
    app::App::run(config, safe_mode)
}
