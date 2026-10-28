import type { ActionState } from "./ActionState.ts";
import type { Context } from "./Context.ts";

import { getWebserverUrl } from "./ports.ts";

import { invoke } from "@tauri-apps/api/core";

export function getImage(image: string | undefined, fallback: string | undefined): string {
	if (!image) return fallback ? getImage(fallback, undefined) : "/alert.png";
	if (image.startsWith("opendeck/")) return image.replace("opendeck", "");
	if (!image.startsWith("data:")) return getWebserverUrl(image);
	const svgxmlre = /^data:image\/svg\+xml(?!.*?;base64.*?)(?:;[\w=]*)*,(.+)/;
	const base64re = /^data:image\/(apng|avif|gif|jpeg|png|svg\+xml|webp|bmp|x-icon|tiff);base64,([A-Za-z0-9+/]+={0,2})?/;
	if (svgxmlre.test(image)) {
		let svg = (svgxmlre.exec(image) as RegExpExecArray)[1].replace(/\;$/, "");
		try {
			svg = decodeURIComponent(svg);
		} finally {
			image = "data:image/svg+xml," + encodeURIComponent(svg);
		}
	}
	if (base64re.test(image)) {
		const exec = base64re.exec(image)!;
		if (!exec[2]) return fallback ? getImage(fallback, undefined) : "/alert.png";
		else image = exec[0];
	}
	return image;
}

export class CanvasLock {
	currentLock = Promise.resolve();
	async lock() {
		let unlockNext: () => void;
		const willLock = new Promise<void>((resolve) => (unlockNext = resolve));
		const previousLock = this.currentLock;
		this.currentLock = willLock;
		await previousLock;
		return unlockNext!;
	}
}

export async function renderImage(
	canvas: HTMLCanvasElement | null,
	slotContext: Context | null,
	state: ActionState,
	fallback: string | undefined,
	showOk: boolean,
	showAlert: boolean,
	processImage: boolean,
	active: boolean,
	pressed: boolean,
	rotation?: number,
) {
	// Create canvas
	let scale = 1;
	if (!canvas) {
		canvas = document.createElement("canvas");
		canvas.width = 144;
		canvas.height = 144;
	} else {
		// Use height for scale to handle rectangular infobar canvases
		scale = canvas.height / 144;
	}

	const context = canvas.getContext("2d");
	if (!context) return;

	context.save();
	if (rotation) {
		context.translate(canvas.width / 2, canvas.height / 2);
		context.rotate((rotation * Math.PI) / 180);
		context.translate(-canvas.width / 2, -canvas.height / 2);
	}

	try {
		// Load image
		const image = document.createElement("img");
		image.crossOrigin = "anonymous";
		const imageSource = state.image_override ?? state.image;
		image.src = processImage ? getImage(imageSource, fallback) : imageSource;
		if (image.src == undefined) return;
		await new Promise((resolve, reject) => {
			image.onload = resolve;
			image.onerror = reject;
		});

		context.clearRect(0, 0, canvas.width, canvas.height);

		// Draw background color
		if (!state.background_colour.startsWith("#000000")) {
			context.fillStyle = state.background_colour;
			context.fillRect(0, 0, canvas.width, canvas.height);
		}

		// Draw image
		context.imageSmoothingQuality = "high";
		const imageScale = Math.max(10, state.image_scale || 100) / 100;
		const xScaled = canvas.width * imageScale;
		const yScaled = canvas.height * imageScale;
		const xOffset = (canvas.width - xScaled) / 2;
		const yOffset = (canvas.height - yScaled) / 2;
		context.drawImage(image, xOffset, yOffset, xScaled, yScaled);
	} catch (error: any) {
		if (!(error instanceof Event)) console.error(error);
		context.clearRect(0, 0, canvas.width, canvas.height);
		showAlert = true;
	}

	// Draw text
	if (state.show) {
		const textSource = state.text_override || state.text;
		const margin = 6 * scale;
		const maxWidth = canvas.width - margin * 2;
		const maxHeight = canvas.height - margin * 2;
		const stylePrefix = (state.style.includes("Bold") ? "bold " : "") + (state.style.includes("Italic") ? "italic " : "");
		const family = `"${state.family}", "Segoe UI Emoji", "Noto Color Emoji", "Apple Color Emoji", sans-serif`;

		// Wrap the title into lines: on spaces where possible, on code points (emoji-safe)
		// when a single word is longer than the button.
		const layoutLines = (fontSize: number) => {
			context.font = stylePrefix + `${fontSize}px ${family}`;
			const lines: Array<{ text: string; width: number }> = [];
			for (const raw of textSource.split("\n")) {
				const words: string[] = [];
				for (const word of raw.split(" ")) {
					if (context.measureText(word).width <= maxWidth || word == "") {
						words.push(word);
						continue;
					}
					let chunk = "";
					for (const symbol of Array.from(word)) {
						if (chunk && context.measureText(chunk + symbol).width > maxWidth) {
							words.push(chunk);
							chunk = symbol;
						} else {
							chunk += symbol;
						}
					}
					if (chunk) words.push(chunk);
				}
				let current = "";
				for (const word of words) {
					const candidate = current && word ? current + " " + word : current || word;
					if (current && context.measureText(candidate).width > maxWidth) {
						lines.push({ text: current, width: context.measureText(current).width });
						current = word;
					} else {
						current = candidate;
					}
				}
				lines.push({ text: current, width: context.measureText(current).width });
			}
			return lines;
		};

		let size = state.size * 2 * scale;
		let lines = layoutLines(size);
		// Auto-fit: shrink the font until the wrapped block fits on the button.
		while ((lines.length * size > maxHeight || lines.some((line) => line.width > maxWidth)) && size > 8 * scale) {
			size = Math.max(8 * scale, size * 0.9);
			lines = layoutLines(size);
		}

		context.textAlign = "center";
		context.fillStyle = state.colour;
		context.strokeStyle = state.stroke_colour;
		context.lineWidth = state.stroke_size * scale;
		context.textBaseline = "top";
		const x = canvas.width / 2;
		const blockHeight = lines.length * size;
		let y = canvas.height / 2 - blockHeight / 2;
		switch (state.alignment) {
			case "top":
				y = margin;
				break;
			case "bottom":
				y = canvas.height - blockHeight - margin;
				break;
		}
		for (const [index, line] of Object.entries(lines)) {
			const lineX = x;
			const lineY = y + size * parseInt(index);
			context.strokeText(line.text, lineX, lineY);
			context.fillText(line.text, lineX, lineY);
			if (state.underline) {
				// Set to black for the outline, since it uses the same fill style info as the text colour.
				context.fillStyle = "black";
				context.fillRect(lineX - line.width / 2 - 3, lineY + size, line.width + 6, 9);
				// Reset to the user's choice of text colour.
				context.fillStyle = state.colour;
				context.fillRect(lineX - line.width / 2, lineY + size + 4, line.width, 3);
			}
		}
	}

	if (showOk) {
		const okImage = document.createElement("img");
		okImage.crossOrigin = "anonymous";
		okImage.src = "/ok.png";
		await new Promise((resolve) => {
			okImage.onload = resolve;
		});
		context.drawImage(okImage, 0, 0, canvas.width, canvas.height);
	}

	if (showAlert) {
		const alertImage = document.createElement("img");
		alertImage.crossOrigin = "anonymous";
		alertImage.src = "/alert.png";
		await new Promise((resolve) => {
			alertImage.onload = resolve;
		});
		context.drawImage(alertImage, 0, 0, canvas.width, canvas.height);
	}

	// Make the image smaller while the button is pressed.
	if (pressed) {
		const smallCanvas = document.createElement("canvas");
		smallCanvas.width = canvas.width;
		smallCanvas.height = canvas.height;
		const newContext = smallCanvas.getContext("2d");
		const margin = 0.1;
		if (newContext) {
			newContext.drawImage(canvas, canvas.width * margin, canvas.height * margin, canvas.width * (1 - margin * 2), canvas.height * (1 - margin * 2));
			context.clearRect(0, 0, canvas.width, canvas.height);
			context.drawImage(smallCanvas, 0, 0);
		}
	}

	context.restore();

	if (active && slotContext) setTimeout(async () => await invoke("update_image", { context: slotContext, image: canvas.toDataURL("image/jpeg") }), 10);
}

export async function resizeImage(source: string): Promise<string | undefined> {
	const canvas = document.createElement("canvas");
	canvas.width = 288;
	canvas.height = 288;
	const context = canvas.getContext("2d");
	if (!context) return;

	const image = document.createElement("img");
	image.crossOrigin = "anonymous";
	image.src = source;
	await new Promise((resolve) => (image.onload = resolve));

	let xOffset = 0,
		yOffset = 0;
	let xScaled = canvas.width,
		yScaled = canvas.height;
	if (image.width > image.height) {
		const ratio = image.height / image.width;
		yScaled = canvas.height * ratio;
		yOffset = (canvas.height - yScaled) / 2;
	} else if (image.width < image.height) {
		const ratio = image.width / image.height;
		xScaled = canvas.width * ratio;
		xOffset = (canvas.width - xScaled) / 2;
	}

	context.imageSmoothingQuality = "high";
	context.clearRect(0, 0, canvas.width, canvas.height);
	context.drawImage(image, xOffset, yOffset, xScaled, yScaled);

	return canvas.toDataURL();
}
