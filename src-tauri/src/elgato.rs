use crate::device_sleep::is_device_sleeping;
use crate::encoder_layouts::generate_encoder_image;
use crate::events::inbound;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use base64::Engine as _;
use elgato_streamdeck::{
	AsyncStreamDeck, DeviceStateUpdate,
	images::{ImageRect, convert_image_with_format_async},
	info::{ImageRotation, Kind},
};
use image::GenericImageView as _;
use tokio::sync::RwLock;

static ELGATO_DEVICES: LazyLock<RwLock<HashMap<String, AsyncStreamDeck>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
static HIDAPI: LazyLock<RwLock<Option<Arc<hidapi::HidApi>>>> = LazyLock::new(|| RwLock::new(None));

/// Extract the average colour from an image.
fn extract_average_colour(img: &image::DynamicImage) -> (u8, u8, u8) {
	let (r_sum, g_sum, b_sum) = img
		.pixels()
		.fold((0u64, 0u64, 0u64), |(r, g, b), (_, _, pixel)| (r + pixel[0] as u64, g + pixel[1] as u64, b + pixel[2] as u64));
	let count = (img.width() * img.height()).max(1) as u64;
	((r_sum / count) as u8, (g_sum / count) as u8, (b_sum / count) as u8)
}

pub async fn update_image(context: &crate::shared::Context, image: Option<&str>) -> Result<(), anyhow::Error> {
	if let Some(device) = ELGATO_DEVICES.read().await.get(&context.device) {
		let kind = device.kind();
		if !kind.is_visual() {
			return Ok(());
		}
		let key_count = kind.key_count();
		let is_touch_point = context.controller == "Keypad" && context.position >= key_count;

		if let Some(image) = image {
			let data = image.split_once(',').unwrap().1;
			let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
			if context.controller == "Encoder" {
				let mut img = generate_encoder_image(context, &bytes).await?;
				remember_screen(context, ScreenEntry::Encoder(img.clone())).await;
				let Some(format) = device.kind().lcd_image_format() else {
					return Err(anyhow::anyhow!("Failed to get LCD image format"));
				};
				img = match format.rotation {
					ImageRotation::Rot0 => img,
					ImageRotation::Rot90 => img.rotate90(),
					ImageRotation::Rot180 => img.rotate180(),
					ImageRotation::Rot270 => img.rotate270(),
				};
				device.write_lcd(context.position as u16 * 200, 0, &ImageRect::from_image_async(img)?).await?;
			} else if context.controller == "Infobar" {
				let img = image::load_from_memory(&bytes)?;
				let Some(format) = device.kind().lcd_image_format() else {
					return Err(anyhow::anyhow!("Failed to get LCD image format"));
				};
				let data = convert_image_with_format_async(format, img.resize_exact(248, 58, image::imageops::FilterType::Lanczos3))?;
				remember_screen(context, ScreenEntry::Infobar(data.clone())).await;
				device.write_lcd_fill(&data).await?;
			} else if is_touch_point {
				let (r, g, b) = extract_average_colour(&image::load_from_memory(&bytes)?);
				remember_screen(context, ScreenEntry::Touchpoint(r, g, b)).await;
				device.set_touchpoint_color(context.position - key_count, r, g, b).await?;
			} else {
				let button_image = image::load_from_memory(&bytes)?;
				remember_screen(context, ScreenEntry::Button(button_image.clone())).await;
				device.set_button_image(context.position, button_image).await?;
			}
		} else if context.controller == "Encoder" {
			let mut img = image::DynamicImage::new_rgb8(200, 100);
			let Some(format) = device.kind().lcd_image_format() else {
				return Err(anyhow::anyhow!("Failed to get LCD image format"));
			};
			img = match format.rotation {
				ImageRotation::Rot0 => img,
				ImageRotation::Rot90 => img.rotate90(),
				ImageRotation::Rot180 => img.rotate180(),
				ImageRotation::Rot270 => img.rotate270(),
			};
			device.write_lcd(context.position as u16 * 200, 0, &ImageRect::from_image_async(img)?).await?;
		} else if context.controller == "Infobar" {
			let Some(format) = device.kind().lcd_image_format() else {
				return Err(anyhow::anyhow!("Failed to get LCD image format"));
			};
			let data = convert_image_with_format_async(format, image::DynamicImage::new_rgb8(248, 58))?;
			device.write_lcd_fill(&data).await?;
		} else if is_touch_point {
			device.set_touchpoint_color(context.position - key_count, 0, 0, 0).await?;
		} else {
			device.clear_button_image(context.position).await?;
		}
		device.flush().await?;
	}
	Ok(())
}

/// Clear all touchpoint LEDs on a device by setting them to black.
async fn clear_all_touchpoints(device: &AsyncStreamDeck) {
	for i in 0..device.kind().touchpoint_count() {
		let _ = device.set_touchpoint_color(i, 0, 0, 0).await;
	}
}

pub async fn clear_screen(id: &str) -> Result<(), anyhow::Error> {
	if let Some(device) = ELGATO_DEVICES.read().await.get(id) {
		device.clear_all_button_images().await?;
		if let Some(lcd_format) = device.kind().lcd_image_format() {
			device
				.write_lcd_fill(&convert_image_with_format_async(
					lcd_format,
					image::DynamicImage::new_rgb8(lcd_format.size.0 as u32, lcd_format.size.1 as u32),
				)?)
				.await?;
		}
		clear_all_touchpoints(device).await;
		device.flush().await?;
	}
	Ok(())
}

/// What the device currently displays for one slot, captured on the way out of update_image.
enum ScreenEntry {
	/// A keypad/button image.
	Button(image::DynamicImage),
	/// An encoder region of the touch LCD, pre-rotation.
	Encoder(image::DynamicImage),
	/// Converted bytes for an infobar fill.
	Infobar(Vec<u8>),
	/// A touchpoint LED colour.
	Touchpoint(u8, u8, u8),
}

/// device -> (profile, page) -> (controller, position) -> last displayed content.
/// Lets a page switch paint the new page from this cache in one buffered HID pass
/// (single flush) instead of clearing to black and repainting over the wire.
static SCREENS: LazyLock<dashmap::DashMap<String, HashMap<(String, u8), HashMap<(String, u8), ScreenEntry>>>> =
	LazyLock::new(dashmap::DashMap::new);

/// Remembers slot content so switching back to a page repaints it instantly.
async fn remember_screen(context: &crate::shared::Context, entry: ScreenEntry) {
	let page = crate::shared::DEVICES
		.get(&context.device)
		.map(|device| device.current_page)
		.unwrap_or(0);
	SCREENS
		.entry(context.device.clone())
		.or_default()
		.entry((context.profile.clone(), page))
		.or_default()
		.insert((context.controller.clone(), context.position), entry);
}

/// Drop all cached pages of one profile (call when pages are removed so indices stay truthful).
pub fn invalidate_profile_screens(device: &str, profile: &str) {
	if let Some(mut pages) = SCREENS.get_mut(device) {
		pages.retain(|(stored_profile, _), _| stored_profile != profile);
	}
}

/// Paints the cached screen for (profile, page), falling back to a full clear.
/// All writes are buffered and flushed once, so the device swaps from the old page
/// straight to the new one without a black flash.
pub async fn restore_or_clear_screen(id: &str, profile: &str, page: u8) -> Result<(), anyhow::Error> {
	let devices = ELGATO_DEVICES.read().await;
	let Some(device) = devices.get(id) else {
		return Ok(());
	};

	// Take (not copy) the cached page; the hardware now owns the content.
	let seed = {
		let mut pages = SCREENS.entry(id.to_owned()).or_default();
		pages.remove(&(profile.to_owned(), page)).unwrap_or_default()
	};

	// Buffer a full wipe first: anything the cache does not cover ends up empty.
	device.clear_all_button_images().await?;
	if let Some(lcd_format) = device.kind().lcd_image_format() {
		device
			.write_lcd_fill(&convert_image_with_format_async(
				lcd_format,
				image::DynamicImage::new_rgb8(lcd_format.size.0 as u32, lcd_format.size.1 as u32),
			)?)
			.await?;
	}
	clear_all_touchpoints(device).await;

	let key_count = device.kind().key_count() as u8;
	for ((_controller, position), entry) in seed {
		let result = match entry {
			ScreenEntry::Button(image) => device.set_button_image(position, image).await,
			ScreenEntry::Encoder(image) => {
				let Some(format) = device.kind().lcd_image_format() else {
					continue;
				};
				let image = match format.rotation {
					ImageRotation::Rot0 => image,
					ImageRotation::Rot90 => image.rotate90(),
					ImageRotation::Rot180 => image.rotate180(),
					ImageRotation::Rot270 => image.rotate270(),
				};
				device.write_lcd(position as u16 * 200, 0, &ImageRect::from_image_async(image)?).await
			}
			ScreenEntry::Infobar(data) => device.write_lcd_fill(&data).await,
			ScreenEntry::Touchpoint(r, g, b) => device.set_touchpoint_color(position - key_count, r, g, b).await,
		};
		if let Err(error) = result {
			log::warn!("Failed to restore cached screen slot: {error}");
		}
	}

	device.flush().await?;
	Ok(())
}


pub async fn set_brightness(id: &str, brightness: u8) {
	if let Some(device) = ELGATO_DEVICES.read().await.get(id) {
		let _ = device.set_brightness(brightness.clamp(0, 100)).await;
		let _ = device.flush().await;
	}
}

pub async fn reset_devices() {
	for device in ELGATO_DEVICES.read().await.values() {
		let _ = device.reset().await;
		let _ = device.flush().await;
	}
}

/// Wake up certain kinds of devices that do not wake themselves after the computer resumes from sleep.
pub async fn wake_deeply_sleeping_devices() {
	for (id, device) in ELGATO_DEVICES.read().await.iter() {
		if device.kind() == Kind::Plus && !is_device_sleeping(id) {
			set_brightness(id, crate::store::get_settings().value.brightness).await;
		}
	}
}

async fn init(device: AsyncStreamDeck, device_id: String) {
	if ELGATO_DEVICES.read().await.contains_key(&device_id) {
		return;
	}

	let device_name = device.product().await.unwrap();
	let kind = device.kind();
	let device_type = match kind {
		Kind::Original | Kind::OriginalV2 | Kind::Mk2 | Kind::Mk2Scissor | Kind::Mk2Module | Kind::Mk2ModuleScissor => 0,
		Kind::Mini | Kind::MiniMk2 | Kind::MiniDiscord | Kind::MiniMk2Module => 1,
		Kind::Xl | Kind::XlV2 | Kind::XlV2Module => 2,
		Kind::Pedal => 5,
		Kind::Plus => 7,
		Kind::Neo => 9,
		Kind::PlusXl => 13,
	};

	let reader = device.get_reader();
	ELGATO_DEVICES.write().await.insert(device_id.clone(), device);
	let _ = clear_screen(&device_id).await;

	crate::events::inbound::devices::register_device(
		"",
		crate::events::inbound::PayloadEvent {
			payload: crate::shared::DeviceInfo {
				id: device_id.clone(),
				plugin: String::new(),
				name: device_name,
				rows: kind.row_count(),
				columns: kind.column_count(),
				encoders: kind.encoder_count(),
				touchpoints: kind.touchpoint_count(),
				infobars: if kind == Kind::Neo { 1 } else { 0 },
				r#type: device_type,
				current_page: 0,
			},
		},
	)
	.await
	.unwrap();

	let press = |position| inbound::PayloadEvent {
		payload: inbound::devices::PressPayload { device: device_id.clone(), position },
	};
	let encoder = |position, ticks: i8| inbound::PayloadEvent {
		payload: inbound::devices::TicksPayload {
			device: device_id.clone(),
			position,
			ticks: ticks.into(),
		},
	};
	let touchscreen_press = |position, x, y, hold| inbound::PayloadEvent {
		payload: inbound::devices::TouchscreenPressPayload {
			device: device_id.clone(),
			position,
			x,
			y,
			hold,
		},
	};
	loop {
		let updates = match reader.read(100.0).await {
			Ok(updates) => updates,
			Err(_) => break,
		};
		for update in updates {
			match match update {
				DeviceStateUpdate::ButtonDown(key) => inbound::devices::key_down(press(key)).await,
				DeviceStateUpdate::ButtonUp(key) => inbound::devices::key_up(press(key)).await,
				DeviceStateUpdate::TouchPointDown(point) => inbound::devices::key_down(press(kind.key_count() + point)).await,
				DeviceStateUpdate::TouchPointUp(point) => inbound::devices::key_up(press(kind.key_count() + point)).await,
				DeviceStateUpdate::EncoderTwist(dial, ticks) => inbound::devices::encoder_change(encoder(dial, ticks)).await,
				DeviceStateUpdate::EncoderDown(dial) => inbound::devices::encoder_down(press(dial)).await,
				DeviceStateUpdate::EncoderUp(dial) => inbound::devices::encoder_up(press(dial)).await,
				DeviceStateUpdate::TouchScreenPress(x, y) => {
					let (position, x, y) = match kind {
						Kind::Plus | Kind::PlusXl => ((x / 200) as u8, x % 200, y),
						_ => continue,
					};
					inbound::devices::touchscreen_press(touchscreen_press(position, x, y, false)).await
				}
				DeviceStateUpdate::TouchScreenLongPress(x, y) => {
					let (position, x, y) = match kind {
						Kind::Plus | Kind::PlusXl => ((x / 200) as u8, x % 200, y),
						_ => continue,
					};
					inbound::devices::touchscreen_press(touchscreen_press(position, x, y, true)).await
				}
				_ => Ok(()),
			} {
				Ok(_) => (),
				Err(error) => log::warn!("Failed to process device event {update:?}: {error}"),
			}
		}
	}

	ELGATO_DEVICES.write().await.remove(&device_id);
	crate::events::inbound::devices::deregister_device("", crate::events::inbound::PayloadEvent { payload: device_id })
		.await
		.unwrap();
}

/// Attempt to initialise all connected devices.
pub async fn initialise_devices() {
	if crate::store::get_settings().value.disableelgato {
		crate::plugins::DEVICE_NAMESPACES
			.write()
			.await
			.insert("sd".to_owned(), "opendeck_alternative_elgato_implementation".to_owned());
		return;
	} else {
		crate::plugins::DEVICE_NAMESPACES.write().await.remove("sd");
	}

	// Iterate through detected Elgato devices and attempt to register them.
	let current = HIDAPI.read().await.as_ref().cloned();
	let hid = match current {
		Some(arc) => arc,
		None => match elgato_streamdeck::new_hidapi() {
			Ok(hid) => {
				let arc = Arc::new(hid);
				HIDAPI.write().await.replace(arc.clone());
				arc
			}
			Err(error) => {
				log::warn!("Failed to initialise hidapi: {error}");
				return;
			}
		},
	};
	for (kind, serial) in elgato_streamdeck::asynchronous::list_devices_async(&hid) {
		let device_id = format!("sd-{serial}");
		if ELGATO_DEVICES.read().await.contains_key(&device_id) {
			continue;
		}
		match elgato_streamdeck::AsyncStreamDeck::connect(&hid, kind, &serial) {
			Ok(device) => {
				tokio::spawn(init(device, device_id));
			}
			Err(error) => log::warn!("Failed to connect to Elgato device: {error}"),
		}
	}
}
