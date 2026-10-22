//! M4 virtual deck: a software-only device rendered in its own window.
//! Registered under the "vd-" namespace so plugins receive the full lifecycle
//! (deviceDidConnect, willAppear, keyDown/keyUp); hardware image writes are
//! naturally no-ops because `update_image` has no handler for this namespace.

use crate::events::inbound;
use crate::shared::{DeviceInfo, DEVICES, PRODUCT_NAME};
use crate::store::profiles::{acquire_locks_mut, get_device_profiles, save_profile_now};

use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const DEVICE_ID: &str = "vd-VIRTUAL0";
const MAX_DIM: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
	rows: u8,
	columns: u8,
}

fn current_layout() -> Layout {
	let settings = crate::store::get_settings().value;
	Layout {
		rows: settings.virtualdeck_rows.clamp(1, MAX_DIM),
		columns: settings.virtualdeck_columns.clamp(1, MAX_DIM),
	}
}

fn profiles_dir() -> PathBuf {
	crate::APP_HANDLE
		.get()
		.unwrap()
		.path()
		.config_dir()
		.unwrap()
		.join("profiles")
		.join(DEVICE_ID)
}

/// Creates or resizes the Default profile so slot lookups never miss bounds.
async fn ensure_layout() -> Result<(), anyhow::Error> {
	let layout = current_layout();
	let dir = profiles_dir();
	fs::create_dir_all(&dir)?;

	let known = get_device_profiles(DEVICE_ID)?.contains(&"Default".to_owned());
	if !known {
		let mut page = crate::shared::Page::default();
		page.keys = vec![None; layout.rows as usize * layout.columns as usize];
		let profile = crate::shared::Profile {
			id: "Default".to_owned(),
			pages: vec![page],
			stale: false,
		};
		fs::write(dir.join("Default.json"), serde_json::to_string_pretty(&profile)?)?;
	}

	// Extend a pre-existing profile if the grid grew.
	let needed = layout.rows as usize * layout.columns as usize;
	let mut locks = acquire_locks_mut().await;
	let device = device_info();
	if let Ok(store) = locks.profile_stores.get_profile_store_mut(&device, "Default").await {
		if store.value.pages.first_mut().unwrap().keys.len() < needed {
			log::info!("Virtual deck: growing Default profile to {} slots", needed);
			store.value.pages.first_mut().unwrap().keys.resize(needed, None);
			let _ = save_profile_now(DEVICE_ID, &mut locks).await;
		}
	}
	Ok(())
}

fn device_info() -> DeviceInfo {
	let layout = current_layout();
	DeviceInfo {
		id: DEVICE_ID.to_owned(),
		plugin: String::new(),
		name: "Virtual Deck".to_owned(),
		rows: layout.rows,
		columns: layout.columns,
		encoders: 0,
		touchpoints: 0,
		infobars: 0,
		r#type: 2,
		current_page: 0,
	}
}

/// Registers (or refreshes) the virtual device, mirroring `elgato::init`.
pub async fn start() {
	if !crate::store::get_settings().value.virtualdeck {
		return;
	}
	if DEVICES.contains_key(DEVICE_ID) {
		return;
	}
	if let Err(error) = ensure_layout().await {
		log::error!("Virtual deck: failed to prepare profiles: {error}");
		return;
	}

	if let Err(error) = crate::events::inbound::devices::register_device(
		"",
		inbound::PayloadEvent { payload: device_info() },
	)
	.await
	{
		log::error!("Virtual deck: failed to register: {error}");
	} else {
		log::info!("Virtual deck registered ({})", DEVICE_ID);
	}
}

/// Deregisters the virtual device, mirroring the disconnect path of `elgato::init`.
pub async fn stop() {
	if !DEVICES.contains_key(DEVICE_ID) {
		return;
	}
	if let Err(error) =
		crate::events::inbound::devices::deregister_device("", inbound::PayloadEvent { payload: DEVICE_ID.to_owned() }).await
	{
		log::error!("Virtual deck: failed to deregister: {error}");
	}
}

/// Reconciles the virtual device after settings changes.
pub async fn apply(old_layout: (bool, u8, u8), new_layout: (bool, u8, u8)) {
	if old_layout == new_layout {
		return;
	}
	if DEVICES.contains_key(DEVICE_ID)
		&& (new_layout.0 != old_layout.0 || new_layout.1 != old_layout.1 || new_layout.2 != old_layout.2)
	{
		stop().await;
	}
	if new_layout.0 {
		start().await;
	}
}

/// Opens the virtual deck window, creating it if needed.
pub async fn open_window(app: &AppHandle) -> Result<(), anyhow::Error> {	if let Some(window) = app.get_webview_window("virtualdeck") {
		let _ = window.show();
		let _ = window.set_focus();
		return Ok(());
	}

	let layout = current_layout();
	let width = layout.columns as f64 * 132.0 + 40.0;
	let height = layout.rows as f64 * 132.0 + 96.0;
	WebviewWindowBuilder::new(app, "virtualdeck", WebviewUrl::App("deck".into()))
		.title(format!("{PRODUCT_NAME} - Virtual Deck"))
		.inner_size(width, height)
		.min_inner_size(width.min(200.0), height.min(170.0))
		.resizable(true)
		.build()?;
	Ok(())
}

#[tauri::command]
pub async fn open_virtual_deck(app: AppHandle) {
	let _ = open_window(&app).await;
}
