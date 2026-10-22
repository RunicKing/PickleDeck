use super::Error;

use crate::shared::{DEVICES, Page};
use crate::store::profiles::{LocksMut, PROFILE_STORES, acquire_locks_mut, get_device_profiles, save_profile_now};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, command};

#[derive(Clone, Serialize, Deserialize)]
pub struct SwitchPageEvent {
	pub device: String,
	pub page: usize,
}

#[command]
pub fn get_profiles(device: &str) -> Result<Vec<String>, Error> {
	Ok(get_device_profiles(device)?)
}

#[command]
pub async fn get_selected_profile(device: String) -> Result<crate::shared::ProfilePage, Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let selected_profile = locks.device_stores.get_selected_profile(&device)?;
	let device_info = DEVICES.get(&device).unwrap();
	let profile = locks.profile_stores.get_profile_store(&device_info, &selected_profile)?;
	let page_index = (device_info.current_page as usize).min(profile.value.pages.len().saturating_sub(1));
	let page = profile.value.pages.get(page_index).ok_or_else(|| Error::new("profile has no pages".to_owned()))?;

	Ok(crate::shared::ProfilePage {
		id: profile.value.id.clone(),
		keys: page.keys.clone(),
		sliders: page.sliders.clone(),
		infobars: page.infobars.clone(),
		pages: profile.value.pages.len(),
	})
}

/// The willDisappear / willAppear special-casing shared by profile and page switching:
/// parent actions (multi action, toggle action, key logic) only dispatch their children.
async fn dispatch_slots(slots: impl Iterator<Item = &crate::shared::ActionInstance>, appear: bool) {
	for instance in slots {
		let is_parent = matches!(instance.action.uuid.as_str(), "opendeck.multiaction" | "opendeck.toggleaction" | "opendeck.keylogic");
		if appear {
			if !is_parent {
				let _ = crate::events::outbound::will_appear::will_appear(instance).await;
			} else {
				for child in instance.children.as_ref().unwrap() {
					let _ = crate::events::outbound::will_appear::will_appear(child).await;
				}
			}
		} else if !is_parent {
			let _ = crate::events::outbound::will_appear::will_disappear(instance, false).await;
		} else {
			for child in instance.children.as_ref().unwrap() {
				let _ = crate::events::outbound::will_appear::will_disappear(child, false).await;
			}
		}
	}
}

/// Sends willDisappear for the page currently mounted on the device.
async fn disappear_current_page(device: &str, profile_id: &str, locks: &mut LocksMut<'_>) -> Result<(), Error> {
	let current = DEVICES.get(device).map(|d| d.current_page as usize).unwrap_or(0);
	let (keys, sliders, infobars) = {
		let store = locks.profile_stores.get_profile_store(&DEVICES.get(device).unwrap(), profile_id)?;
		let index = current.min(store.value.pages.len().saturating_sub(1));
		match store.value.pages.get(index) {
			Some(page) => (page.keys.clone(), page.sliders.clone(), page.infobars.clone()),
			None => return Ok(()),
		}
	};
	dispatch_slots(keys.iter().flatten().chain(sliders.iter().flatten()).chain(infobars.iter().flatten()), false).await;
	Ok(())
}

/// Sends willAppear for the given page of a profile (store is created if missing).
async fn appear_page(device: &str, profile_id: &str, page_index: usize, locks: &mut LocksMut<'_>) -> Result<(), Error> {
	let (keys, sliders, infobars) = {
		let store = locks.profile_stores.get_profile_store_mut(&DEVICES.get(device).unwrap(), profile_id).await?;
		let index = page_index.min(store.value.pages.len().saturating_sub(1));
		match store.value.pages.get(index) {
			Some(page) => (page.keys.clone(), page.sliders.clone(), page.infobars.clone()),
			None => return Ok(()),
		}
	};
	dispatch_slots(keys.iter().flatten().chain(sliders.iter().flatten()).chain(infobars.iter().flatten()), true).await;
	Ok(())
}

/// Mounts the given page of the selected profile: willDisappear → clear screen →
/// switch → willAppear → persistence; emits `switch_page` to the frontend.
async fn set_page_flow(app: &AppHandle, device: &str, page: usize, locks: &mut LocksMut<'_>) -> Result<(), Error> {
	let profile_id = locks.device_stores.get_selected_profile(device)?;

	disappear_current_page(device, &profile_id, locks).await?;
	let _ = crate::events::outbound::devices::clear_screen(device.to_owned()).await;

	DEVICES.get_mut(device).unwrap().current_page = page as u8;
	locks.device_stores.set_selected_page(device, page as u8)?;

	appear_page(device, &profile_id, page, locks).await?;

	let _ = app.emit(
		"switch_page",
		SwitchPageEvent {
			device: device.to_owned(),
			page,
		},
	);
	Ok(())
}

#[allow(clippy::flat_map_identity)]
#[command]
pub async fn set_selected_profile(device: String, id: String) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	// If a profile save is pending for this device, save it immediately to prevent losing profile data
	if let Err(error) = save_profile_now(&device, &mut locks).await {
		log::error!("Failed to save profile for device {device}: {error}");
	}

	let selected_profile = locks.device_stores.get_selected_profile(&device)?;

	if selected_profile != id {
		disappear_current_page(&device, &selected_profile, &mut locks).await?;
		let _ = crate::events::outbound::devices::clear_screen(device.clone()).await;

		// Profile switches always reset to the first page of the new profile. Re-selecting the
		// current profile (frontend init) keeps the device's restored page selection.
		DEVICES.get_mut(&device).unwrap().current_page = 0;
		locks.device_stores.set_selected_page(&device, 0)?;

		appear_page(&device, &id, 0, &mut locks).await?;
	}

	locks.device_stores.set_selected_profile(&device, id)?;

	Ok(())
}

#[command]
pub async fn set_selected_page(app: AppHandle, device: String, page: u8) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let page_count = {
		let profile_id = locks.device_stores.get_selected_profile(&device)?;
		let store = locks.profile_stores.get_profile_store(&DEVICES.get(&device).unwrap(), &profile_id)?;
		store.value.pages.len()
	};
	if page_count == 0 {
		return Err(Error::new("profile has no pages".to_owned()));
	}
	let page = (page as usize).min(page_count - 1);
	if DEVICES.get(&device).unwrap().current_page as usize == page {
		return Ok(());
	}

	// If a profile save is pending for this device, save it immediately to prevent losing profile data
	if let Err(error) = save_profile_now(&device, &mut locks).await {
		log::error!("Failed to save profile for device {device}: {error}");
	}

	set_page_flow(&app, &device, page, &mut locks).await
}

#[command]
pub async fn add_page(app: AppHandle, device: String) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let profile_id = locks.device_stores.get_selected_profile(&device)?;
	let new_index = {
		let device_info = DEVICES.get(&device).unwrap();
		let store = locks.profile_stores.get_profile_store_mut(&device_info, &profile_id).await?;
		if store.value.pages.len() >= u8::MAX as usize {
			return Err(Error::new("too many pages".to_owned()));
		}

		let mut page = Page::default();
		page.keys.resize((device_info.rows * device_info.columns + device_info.touchpoints) as usize, None);
		page.sliders.resize(device_info.encoders as usize, None);
		page.infobars.resize(device_info.infobars as usize, None);
		store.value.pages.push(page);
		store.save()?;
		store.value.pages.len() - 1
	};

	set_page_flow(&app, &device, new_index, &mut locks).await
}

#[command]
pub async fn remove_page(app: AppHandle, device: String, page: u8) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let profile_id = locks.device_stores.get_selected_profile(&device)?;
	let current = DEVICES.get(&device).unwrap().current_page as usize;

	// If the mounted page is being removed, send willDisappear for it first
	if page as usize == current {
		disappear_current_page(&device, &profile_id, &mut locks).await?;
		let _ = crate::events::outbound::devices::clear_screen(device.clone()).await;
	}

	let new_index = {
		let store = locks.profile_stores.get_profile_store_mut(&DEVICES.get(&device).unwrap(), &profile_id).await?;
		let removed = page as usize;
		if store.value.pages.len() <= 1 {
			return Ok(());
		}
		if removed >= store.value.pages.len() {
			return Err(Error::new("page out of bounds".to_owned()));
		}
		store.value.pages.remove(removed);
		store.save()?;

		// Adjust the mounted page index to follow the removal
		if removed < current {
			current - 1
		} else if page as usize == current {
			0
		} else {
			current
		}
	};

	DEVICES.get_mut(&device).unwrap().current_page = new_index as u8;
	locks.device_stores.set_selected_page(&device, new_index as u8)?;

	if page as usize == current {
		appear_page(&device, &profile_id, new_index, &mut locks).await?;
	}

	let _ = app.emit(
		"switch_page",
		SwitchPageEvent {
			device,
			page: new_index,
		},
	);
	Ok(())
}

#[command]
pub async fn delete_profile(device: String, profile: String) {
	let mut profile_stores = PROFILE_STORES.write().await;
	profile_stores.delete_profile(&device, &profile);
}

#[command]
pub async fn rename_profile(device: String, old_id: String, new_id: String, retain: bool) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	locks.profile_stores.rename_profile(&DEVICES.get(&device).unwrap(), &old_id, &new_id, retain).await?;

	Ok(())
}

pub async fn rerender_images(app: &AppHandle) -> Result<(), anyhow::Error> {
	let _ = app.emit("rerender_images", ());
	Ok(())
}
