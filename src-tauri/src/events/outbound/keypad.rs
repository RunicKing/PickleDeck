use super::{GenericInstancePayload, send_to_plugin};

use crate::events::frontend::instances::{key_moved, update_state};
use crate::shared::{ActionContext, Context};
use crate::store::profiles::{acquire_locks_mut, get_slot_mut, mark_profile_stale};

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use serde::Serialize;

static KEY_DOWN_TARGETS: LazyLock<DashMap<(String, u8), Context>> = LazyLock::new(DashMap::new);

/// Key Logic: (device, key) -> (press sequence number, profile the press started under,
/// double-press window ms). Present while a keylogic key is held and the long-press branch
/// has not fired yet.
static KEY_HOLD: LazyLock<DashMap<(String, u8), (u64, String, u64)>> = LazyLock::new(DashMap::new);
/// Key Logic: (device, key) -> (sequence of the pending single press, profile).
/// A keylogic press whose tap branch waits for a possible second press (double press).
static KEY_DOUBLES: LazyLock<DashMap<(String, u8), (u64, String)>> = LazyLock::new(DashMap::new);
static KEY_HOLD_SEQ: AtomicU64 = AtomicU64::new(0);
const KEYLOGIC_DEFAULT_HOLD: u64 = 500;
const KEYLOGIC_DEFAULT_DOUBLE: u64 = 275;

const PAGE_ACTIONS: [&str; 3] = ["opendeck.nextpage", "opendeck.prevpage", "opendeck.gotopage"];

/// Handles the built-in page navigation actions (Next/Previous/Go to Page): resolves the
/// target page against the current profile and runs the standard page switch flow.
async fn trigger_page_change(device: &str, uuid: &str, page: Option<u8>) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	if !crate::shared::DEVICES.contains_key(device) {
		return Ok(());
	}
	let profile_id = locks.device_stores.get_selected_profile(device)?;
	let page_count = {
		let store = locks
			.profile_stores
			.get_profile_store(&crate::shared::DEVICES.get(device).unwrap(), &profile_id)?;
		store.value.pages.len()
	} as u32;
	if page_count == 0 {
		return Ok(());
	}
	let current = crate::shared::DEVICES.get(device).unwrap().current_page as u32;

	let target = match uuid {
		"opendeck.nextpage" => (current + 1) % page_count,
		"opendeck.prevpage" => (current + page_count - 1) % page_count,
		// Unset target defaults to page 1; a target pointing outside the profile (e.g. after
		// page count changes) does nothing.
		_ => {
			let target = page.map(|page| page as u32).unwrap_or(0);
			if target >= page_count {
				log::debug!("Go to Page target {} out of range ({} pages)", target, page_count);
				return Ok(());
			}
			target
		}
	};

	crate::events::frontend::profiles::set_page_flow(crate::APP_HANDLE.get().unwrap(), device, target as usize, &mut locks)
		.await?;
	Ok(())
}

/// Fires one branch of a keylogic instance: index 0 = single press, 1 = double press, 2 = long press.
/// Mirrors the multiaction child dispatch plus its automatic state cycling.
/// Runs without holding the profile locks across the plugin round-trip.
async fn fire_keylogic_branch(device: String, key: u8, profile: String, index: usize) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	let context = Context {
		device: device.clone(),
		profile: profile.clone(),
		controller: "Keypad".to_owned(),
		position: key,
	};
	let Some(instance) = get_slot_mut(&context, &mut locks).await? else { return Ok(()) };
	if instance.action.uuid != "opendeck.keylogic" {
		return Ok(());
	}
	let children = instance.children.clone().unwrap_or_default();
	drop(locks);

	// Branch identity is the child's assigned context index (its slot), not array position,
	// so holes created by removing middle slots keep remaining branches stable.
	let Some(child) = children.iter().find(|c| c.context.index as usize == index) else { return Ok(()) };

	send_to_plugin(
		&child.action.plugin,
		&KeyEvent {
			event: "keyDown",
			action: child.action.uuid.clone(),
			context: child.context.clone(),
			device: child.context.device.clone(),
			payload: GenericInstancePayload::new(child),
		},
	)
	.await?;

	tokio::time::sleep(Duration::from_millis(100)).await;

	send_to_plugin(
		&child.action.plugin,
		&KeyEvent {
			event: "keyUp",
			action: child.action.uuid.clone(),
			context: child.context.clone(),
			device: child.context.device.clone(),
			payload: GenericInstancePayload::new(child),
		},
	)
	.await?;

	// Cycle the fired child's automatic state and refresh the UI.
	// NOTE: unlike the plain dispatch paths we cycle even when the child sets
	// disable_automatic_states, because some plugins only report setState feedback
	// for one registered context per action uuid; keylogic children then never
	// receive state updates from the plugin and every press would repeat the
	// same action (e.g. mute without unmute). Alternating the state here gives
	// the plugin the toggle signal it derives from the payload state.
	let mut locks = acquire_locks_mut().await;
	let refreshed: Option<crate::shared::ActionContext> = match get_slot_mut(&context, &mut locks).await? {
		Some(instance) if instance.action.uuid == "opendeck.keylogic" => {
			instance.children.as_mut().and_then(|children| {
				let child = children.iter_mut().find(|c| c.context.index as usize == index)?;
				if child.states.len() == 2 {
					child.current_state = (child.current_state + 1) % 2;
				}
				Some(child.context.clone())
			})
		}
		_ => None,
	};
	if let Some(context) = refreshed {
		let _ = update_state(crate::APP_HANDLE.get().unwrap(), context, &mut locks).await;
	}
	mark_profile_stale(&device, &mut locks).await?;
	Ok(())
}

#[derive(Serialize)]
struct KeyEvent {
	event: &'static str,
	action: String,
	context: ActionContext,
	device: String,
	payload: GenericInstancePayload,
}

pub async fn key_down(device: &str, key: u8) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	let selected_profile = locks.device_stores.get_selected_profile(device)?;
	let context = Context {
		device: device.to_owned(),
		profile: selected_profile.to_owned(),
		controller: "Keypad".to_owned(),
		position: key,
	};

	let _ = key_moved(crate::APP_HANDLE.get().unwrap(), context.clone(), true).await;
	KEY_DOWN_TARGETS.insert((device.to_owned(), key), context.clone());

	let Some(instance) = get_slot_mut(&context, &mut locks).await? else { return Ok(()) };
	if instance.action.uuid == "opendeck.multiaction" {
		let children = instance.children.clone().unwrap_or_default();
		let delays: Vec<u64> = instance
			.settings
			.get("delays")
			.and_then(|v| v.as_array())
			.map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
			.unwrap_or_default();

		drop(locks);

		for (i, child) in children.iter().enumerate() {
			send_to_plugin(
				&child.action.plugin,
				&KeyEvent {
					event: "keyDown",
					action: child.action.uuid.clone(),
					context: child.context.clone(),
					device: child.context.device.clone(),
					payload: GenericInstancePayload::new(child),
				},
			)
			.await?;

			tokio::time::sleep(Duration::from_millis(100)).await;

			send_to_plugin(
				&child.action.plugin,
				&KeyEvent {
					event: "keyUp",
					action: child.action.uuid.clone(),
					context: child.context.clone(),
					device: child.context.device.clone(),
					payload: GenericInstancePayload::new(child),
				},
			)
			.await?;

			let delay = delays.get(i).copied().unwrap_or(100);
			if delay > 0 {
				tokio::time::sleep(Duration::from_millis(delay)).await;
			}
		}

		let mut locks = acquire_locks_mut().await;

		if let Some(instance) = get_slot_mut(&context, &mut locks).await?
			&& let Some(children) = &mut instance.children
		{
			for child in &mut *children {
				if child.states.len() == 2 && !child.action.disable_automatic_states {
					child.current_state = (child.current_state + 1) % (child.states.len() as u16);
				}
			}

			for child in children.iter().map(|x| x.context.clone()).collect::<Vec<_>>() {
				let _ = update_state(crate::APP_HANDLE.get().unwrap(), child, &mut locks).await;
			}
		}

		mark_profile_stale(device, &mut locks).await?;
	} else if PAGE_ACTIONS.contains(&instance.action.uuid.as_str()) {
		let uuid = instance.action.uuid.clone();
		let page = instance.settings.get("page").and_then(|v| v.as_u64()).map(|v| v as u8);
		drop(locks);
		trigger_page_change(device, &uuid, page).await?;
		return Ok(());
	} else if instance.action.uuid == "opendeck.toggleaction" {		let children = instance.children.as_ref().unwrap();
		if children.is_empty() {
			return Ok(());
		}
		let child = &children[instance.current_state as usize];
		send_to_plugin(
			&child.action.plugin,
			&KeyEvent {
				event: "keyDown",
				action: child.action.uuid.clone(),
				context: child.context.clone(),
				device: child.context.device.clone(),
				payload: GenericInstancePayload::new(child),
			},
		)
		.await?;
	} else if instance.action.uuid == "opendeck.keylogic" {
		// Do not dispatch anything yet; the press, double press or long-press branch fires later.
		let hold_ms = instance
			.settings
			.get("hold")
			.and_then(|v| v.as_u64())
			.filter(|v| (50..=5000).contains(v))
			.unwrap_or(KEYLOGIC_DEFAULT_HOLD);
		let double_ms = instance
			.settings
			.get("double")
			.and_then(|v| v.as_u64())
			.filter(|v| (50..=2000).contains(v))
			.unwrap_or(KEYLOGIC_DEFAULT_DOUBLE);
		drop(locks);

		let device = device.to_owned();
		let seq = KEY_HOLD_SEQ.fetch_add(1, Ordering::SeqCst);
		KEY_HOLD.insert((device.to_owned(), key), (seq, context.profile.clone(), double_ms));
		tokio::spawn(async move {
			tokio::time::sleep(Duration::from_millis(hold_ms)).await;
			let Some(entry) = KEY_HOLD.get(&(device.clone(), key)) else { return };
			if entry.value().0 != seq {
				return;
			}
			let profile = entry.value().1.clone();
			drop(entry);
			KEY_HOLD.remove(&(device.clone(), key));
			if let Err(error) = fire_keylogic_branch(device.clone(), key, profile, 2).await {
				log::warn!("Key Logic long press failed: {}", error);
			}
		});
	} else {
		send_to_plugin(
			&instance.action.plugin,
			&KeyEvent {
				event: "keyDown",
				action: instance.action.uuid.clone(),
				context: instance.context.clone(),
				device: instance.context.device.clone(),
				payload: GenericInstancePayload::new(instance),
			},
		)
		.await?;
	}

	Ok(())
}

pub async fn key_up(device: &str, key: u8) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	let selected_profile = locks.device_stores.get_selected_profile(device)?;
	let context = Context {
		device: device.to_owned(),
		profile: selected_profile.to_owned(),
		controller: "Keypad".to_owned(),
		position: key,
	};

	let _ = key_moved(crate::APP_HANDLE.get().unwrap(), context.clone(), false).await;
	let Some((_, expected_context)) = KEY_DOWN_TARGETS.remove(&(device.to_owned(), key)) else {
		return Ok(());
	};
	if context != expected_context {
		// profile switched while the key was held; cancel any pending branches
		KEY_HOLD.remove(&(device.to_owned(), key));
		KEY_DOUBLES.remove(&(device.to_owned(), key));
		return Ok(());
	}

	// keylogic fast-path: clone the uuid then release locks before dispatching,
	// so the profile write lock is not held across the plugin round-trip.
	let slot_uuid = get_slot_mut(&context, &mut locks).await?.as_ref().map(|instance| instance.action.uuid.clone());
	if slot_uuid.as_deref() == Some("opendeck.keylogic") {
		let device = device.to_owned();
		drop(locks);
		if let Some(entry) = KEY_HOLD.remove(&(device.clone(), key)) {
			let (_, profile, double_ms) = entry.1;
			if KEY_DOUBLES.remove(&(device.clone(), key)).is_some() {
				// second release inside the double window => double press branch
				if let Err(error) = fire_keylogic_branch(device.clone(), key, profile, 1).await {
					log::warn!("Key Logic double press failed: {}", error);
				}
			} else {
				// first release: hold the press open for the double window, then fire
				// the single press branch if no second press arrives in time.
				let seq = KEY_HOLD_SEQ.fetch_add(1, Ordering::SeqCst);
				KEY_DOUBLES.insert((device.clone(), key), (seq, profile.clone()));
				tokio::spawn(async move {
					tokio::time::sleep(Duration::from_millis(double_ms)).await;
					let Some(pending) = KEY_DOUBLES.get(&(device.clone(), key)) else { return };
					if pending.value().0 != seq {
						return;
					}
					let profile = pending.value().1.clone();
					drop(pending);
					KEY_DOUBLES.remove(&(device.clone(), key));
					if let Err(error) = fire_keylogic_branch(device.clone(), key, profile, 0).await {
						log::warn!("Key Logic single press failed: {}", error);
					}
				});
			}
		}
		// no hold entry means the long-press timer already fired the long branch
		return Ok(());
	}

	// Built-in page actions have no plugin process; skip the plugin keyUp dispatch.
	if slot_uuid.as_deref().is_some_and(|uuid| PAGE_ACTIONS.contains(&uuid)) {
		return Ok(());
	}

	let slot = get_slot_mut(&context, &mut locks).await?;
	let Some(instance) = slot else { return Ok(()) };

	if instance.action.uuid == "opendeck.toggleaction" {
		let index = instance.current_state as usize;
		let children = instance.children.as_ref().unwrap();
		if children.is_empty() {
			return Ok(());
		}
		let child = &children[index];
		send_to_plugin(
			&child.action.plugin,
			&KeyEvent {
				event: "keyUp",
				action: child.action.uuid.clone(),
				context: child.context.clone(),
				device: child.context.device.clone(),
				payload: GenericInstancePayload::new(child),
			},
		)
		.await?;
		instance.current_state = ((index + 1) % instance.children.as_ref().unwrap().len()) as u16;
	} else if instance.action.uuid != "opendeck.multiaction" {
		if instance.states.len() == 2 && !instance.action.disable_automatic_states {
			instance.current_state = (instance.current_state + 1) % (instance.states.len() as u16);
		}
		send_to_plugin(
			&instance.action.plugin,
			&KeyEvent {
				event: "keyUp",
				action: instance.action.uuid.clone(),
				context: instance.context.clone(),
				device: instance.context.device.clone(),
				payload: GenericInstancePayload::new(instance),
			},
		)
		.await?;
	};

	let _ = update_state(crate::APP_HANDLE.get().unwrap(), instance.context.clone(), &mut locks).await;
	mark_profile_stale(device, &mut locks).await?;

	Ok(())
}
