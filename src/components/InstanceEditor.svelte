<script lang="ts">
	import type { ActionInstance } from "$lib/ActionInstance";

	import { t } from "$lib/i18n";
	import { renderImage, resizeImage } from "$lib/rendererHelper";

	import { invoke } from "@tauri-apps/api/core";
	import { onMount, tick } from "svelte";

	export let instance: ActionInstance;
	export let showEditor: boolean;

	let state: number = 0;
	let bold: boolean;
	let italic: boolean;

	let fonts: string[] = [];
	onMount(async () => {
		fonts = await invoke("get_fonts");
	});

	let fileInput: HTMLInputElement;
	let solidColourInput: HTMLInputElement;
	let backgroundColourInput: HTMLInputElement;

	let textInput: HTMLTextAreaElement;
	let showEmojiPicker = false;
	// Enough everyday emoji to cover most button-title use; the textarea itself accepts anything else.
	const EMOJIS = [
		"😀","😆","😅","😂","🥹","😊","😍","🤩","😘","😎","🤔","🥳","😴","😭","😡","🤯",
		"🥰","😇","🙃","😱","🤗","🤫","🫡","🤤","😷","🤒","🥺","😤","🤠","🤪","🧐","😬",
		"👍","👎","👌","🤝","🙏","👏","💪","🫶","✌️","🤞","👋","🫡","🖐️","✊","☝️","🤙",
		"❤️","🧡","💛","💚","💙","💜","🖤","🤍","💯","✨","⚡","🔥","💥","💫","⭐","🌟",
		"✅","❌","❓","❗","💬","💤","📝","📌","📎","🔒","🔑","⚠️","🚫","🔄","➡️","⬅️",
		"⏰","📅","📞","✉️","🎮","🎧","🎬","📷","🖥️","💡","🚀","🏁","🏆","🎯","🎉","🎁",
		"🍕","🍔","☕","🍺","🎂","🧠","👀","🤖","🌈","☀️","🌙","☁️","❄️","🌊","🍀","🌸",
	];

	function insertEmoji(emoji: string) {
		// Assign through the reactive chain (instance.states[state], not an alias): Svelte only
		// invalidates assignments written against the component variable itself.
		const current = instance.states[state].text_override ?? "";
		if (textInput) {
			const start = textInput.selectionStart ?? current.length;
			const end = textInput.selectionEnd ?? start;
			const next = current.slice(0, start) + emoji + current.slice(end);
			instance.states[state].text_override = next;
			// Post-assignment self-assign so the send guard reactive block reruns (see below).
			instance = instance;
			// Field is focused, so the echo-sync block skips it; keep the DOM in step ourselves.
			textInput.value = next;
			const caret = start + emoji.length;
			tick().then(() => {
				textInput.focus();
				textInput.setSelectionRange(caret, caret);
			});
		} else {
			instance.states[state].text_override = current + emoji;
			instance = instance;
		}
	}

	function onTextInput() {
		// Assign through the reactive chain + self-assign `instance`: Svelte only invalidates
		// chained writes via a local variable reassignment, and the send guard below depends
		// on `instance`.
		instance.states[state].text_override = textInput.value;
		instance = instance;
	}

	// The backend echoes every edit back via update_state, which replaces `instance`. Rewriting
	// textarea.value mid-typing (stale echo vs faster keystrokes) reverts+loses characters, so
	// outside values are only pushed into the field while it is NOT focused. When the user
	// blurs or the next echo/state-change fires, it catches up.
	$: if (textInput && document.activeElement !== textInput) {
		textInput.value = instance.states[state].text_override ?? "";
	}

	function adjustImageScale(delta: number) {
		const next = (instance.states[state].image_scale || 100) + delta;
		instance.states[state].image_scale = Math.max(10, Math.min(200, next));
	}

	function handleDrop(event: DragEvent) {
		event.preventDefault();

		const file = event.dataTransfer?.files?.[0];
		if (!file || !file.type.startsWith("image/")) return;
		const reader = new FileReader();

		reader.onload = async () => {
			let result = reader.result?.toString();
			if (result) {
				let resized = await resizeImage(result);
				if (resized) instance.states[state].image_override = resized;
				else instance.states[state].image_override = result;
			}
		};

		reader.readAsDataURL(file);
	}

	// Structural probe: log every render/value sweep of instance.states[state].text_override
	function update(instance: ActionInstance) {
		bold = instance.states[state].style.includes("Bold");
		italic = instance.states[state].style.includes("Italic");
	}
	$: update(instance);
	// Only send edits that changed content: the backend echoes this slot back via
	// update_state (to repaint the device), so an identity-only change must not be
	// re-sent or the echo loop would spin save+emit on every keystroke.
	let lastStateSent = "";
	// Resync the dedup key ONLY on identity change: an echo from the backend replaces
	// `instance` wholesale (new object), while local typing reassigns the same object, so
	// comparing identity leaves the guard free to fire on keystrokes and still lets a
	// stale echo die instead of bouncing back into another (send-echo) loop iteration.
	let lastInstance = null;
	$: if (instance !== lastInstance) {
		lastInstance = instance;
		lastStateSent = JSON.stringify(instance.states[state]);
	}
	$: {
		const serialized = JSON.stringify(instance.states[state]);
		if (serialized != lastStateSent) {
			lastStateSent = serialized;
			invoke("set_state", { context: instance.context, index: state, state: instance.states[state] });
		}
	}

	let canvas: HTMLCanvasElement;
	$: renderImage(canvas, null, instance.states[state], instance.action.states[state]?.image ?? instance.action.icon, false, false, true, false, false, 0);
</script>

<svelte:window
	on:keydown={(event) => {
		if (event.key == "Escape") showEditor = false;
	}}
/>

<div class="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 p-2 text-neutral-300 bg-neutral-700 border border-neutral-600 rounded-lg z-10">
	<div class="flex flex-row">
		<div class="select-wrapper m-1 w-full">
			<select class="w-full bg-neutral-600! border-neutral-500!" bind:value={state} aria-label={$t("instance_editor.state")}>
				{#each instance.states as _, i}
					<option value={i}>{$t("instance_editor.state.n", { n: i + 1 })}</option>
				{/each}
			</select>
		</div>
		<button class="ml-2 mr-1 float-right text-xl text-neutral-300" on:click={() => (showEditor = false)} aria-label={$t("settings.close")}>✕</button>
	</div>
	<div class="flex flex-row mx-1">
		<div class="flex flex-col justify-center items-center mt-2 mb-1">
			<button
				on:click={(event) => {
					if (event.ctrlKey) return;
					fileInput.click();
				}}
				on:dragover={(event) => {
					event.preventDefault();
					if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
				}}
				on:drop={handleDrop}
				on:contextmenu={(event) => {
					event.preventDefault();
					if (instance.states[state].image_override) instance.states[state].image_override = null;
					// Support further resetting the base image to the manifest value as a migration path
					// for profiles where the base image and user override image were indistinguishable.
					else instance.states[state].image = instance.action.states[state]?.image ?? instance.action.icon;
				}}
				title={$t("instance_editor.image.hint")}
				aria-label={$t("instance_editor.image.hint")}
			>
				<canvas
					bind:this={canvas}
					class="bg-black border border-neutral-600 rounded-xl cursor-pointer"
					width={144}
					height={144}
					style={`transform: scale(${128 / 144}); margin: ${(128 - 144) / 2}px;`}
					aria-label={$t("instance_editor.image.n", { n: state + 1 })}
				/>
			</button>
			<div class="flex flex-row items-center justify-center mt-1 space-x-1 text-neutral-300">
				<button
					on:click={() => adjustImageScale(-10)}
					class="w-6 h-6 text-sm bg-neutral-600 hover:bg-neutral-500 transition-colors border border-neutral-500 rounded-md"
					title={$t("instance_editor.image.scale.decrease")}
					aria-label={$t("instance_editor.image.scale.decrease")}
				>
					-
				</button>
				<span class="min-w-12 text-center text-xs tabular-nums">
					{instance.states[state].image_scale || 100}%
				</span>
				<button
					on:click={() => adjustImageScale(10)}
					class="w-6 h-6 text-sm bg-neutral-600 hover:bg-neutral-500 transition-colors border border-neutral-500 rounded-md"
					title={$t("instance_editor.image.scale.increase")}
					aria-label={$t("instance_editor.image.scale.increase")}
				>
					+
				</button>
			</div>
			<button
				on:click={() => backgroundColourInput.click()}
				on:focus={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) backgroundColourInput.className = "";
				}}
				on:mouseover={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) backgroundColourInput.className = "";
				}}
				on:blur={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) backgroundColourInput.className = "absolute invisible w-0 h-0";
				}}
				on:mouseleave={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) backgroundColourInput.className = "absolute invisible w-0 h-0";
				}}
				class="mt-1 px-0.5 text-sm text-neutral-300 bg-neutral-600 hover:bg-neutral-500 transition-colors border border-neutral-500 rounded-lg"
			>
				{$t("instance_editor.background")}
				<input bind:this={backgroundColourInput} type="color" bind:value={instance.states[state].background_colour} class="absolute invisible w-0 h-0" />
			</button>
			<button
				on:click={() => solidColourInput.click()}
				on:focus={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) solidColourInput.className = "";
				}}
				on:mouseover={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) solidColourInput.className = "";
				}}
				on:blur={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) solidColourInput.className = "absolute invisible w-0 h-0";
				}}
				on:mouseleave={() => {
					if (navigator.userAgent.toLowerCase().includes("mac")) solidColourInput.className = "absolute invisible w-0 h-0";
				}}
				class="mt-1 px-0.5 text-sm text-neutral-300 bg-neutral-600 hover:bg-neutral-500 transition-colors border border-neutral-500 rounded-lg"
			>
				{$t("instance_editor.solid_colour")}
				<input
					bind:this={solidColourInput}
					type="color"
					class="absolute invisible w-0 h-0"
					value="#FFFFFE"
					on:change={() => {
						const canvas = document.createElement("canvas");
						canvas.width = 1;
						canvas.height = 1;
						const context = canvas.getContext("2d");
						if (!context) return;
						context.fillStyle = solidColourInput.value;
						context.fillRect(0, 0, canvas.width, canvas.height);
						instance.states[state].image_override = canvas.toDataURL("image/png");
					}}
				/>
			</button>
		</div>
		<input
			bind:this={fileInput}
			type="file"
			class="hidden"
			accept="image/*"
			on:change={async () => {
				if (!fileInput.files || fileInput.files.length == 0) return;
				const reader = new FileReader();

				reader.onload = async () => {
					let result = reader.result?.toString();
					if (result) {
						let resized = await resizeImage(result);
						if (resized) instance.states[state].image_override = resized;
						else instance.states[state].image_override = result;
					}
				};

				reader.readAsDataURL(fileInput.files[0]);
			}}
		/>

		<div class="flex flex-col justify-center pl-4 pr-2 pt-4 pb-2 space-y-2">
			<div class="flex flex-row items-start space-x-2">
				<label for="editor-text" class="mt-1">{$t("instance_editor.text")}</label>
				<div class="relative w-full">
					<textarea
						bind:this={textInput}
						on:input={onTextInput}
						placeholder={instance.states[state].text}
						rows="2"
						class="w-full px-1 text-neutral-300 bg-neutral-600 border border-neutral-500 rounded-lg resize-none"
						id="editor-text"
					/>
					<button
						type="button"
						class="absolute bottom-1 right-1 px-1 text-sm bg-neutral-700 hover:bg-neutral-500 transition-colors border border-neutral-500 rounded-md"
						title="Emoji"
						aria-label="Insert emoji"
						on:click={() => (showEmojiPicker = !showEmojiPicker)}
					>
						😀
					</button>
				</div>
			</div>
			{#if showEmojiPicker}
				<div class="grid max-h-36 grid-cols-16 gap-1 overflow-y-auto p-2 bg-neutral-800 border border-neutral-600 rounded-lg">
					{#each EMOJIS as emoji}
						<button type="button" class="px-1 py-0.5 text-lg rounded hover:bg-neutral-600 transition-colors" on:click={() => insertEmoji(emoji)}>
							{emoji}
						</button>
					{/each}
				</div>
			{/if}
			<div class="flex flex-row items-center">
				<label for="editor-colour" class="mr-2">{$t("instance_editor.colour")}</label>
				<input
					type="color"
					bind:value={instance.states[state].colour}
					class="mr-2 px-0.5 bg-neutral-600 border border-neutral-500 rounded-lg"
					id="editor-colour"
				/>
				<label for="editor-show" class="mr-2">{$t("instance_editor.show")}</label>
				<input type="checkbox" bind:checked={instance.states[state].show} class="mr-4 mt-1 scale-125" id="editor-show" />
				<select bind:value={instance.states[state].alignment} class="px-1! py-0.5!" aria-label={$t("instance_editor.alignment")}>
					<option value="top">{$t("instance_editor.alignment.top")}</option>
					<option value="middle">{$t("instance_editor.alignment.middle")}</option>
					<option value="bottom">{$t("instance_editor.alignment.bottom")}</option>
				</select>
			</div>
			<div class="flex flex-row items-center">
				<label for="editor-stroke" class="mr-2">{$t("instance_editor.stroke")}</label>
				<input
					type="color"
					bind:value={instance.states[state].stroke_colour}
					class="mr-2 px-0.5 bg-neutral-600 border border-neutral-500 rounded-lg"
					id="editor-stroke"
				/>
				<label for="editor-outline" class="mr-2">{$t("instance_editor.outline")}</label>
				<input
					type="number"
					bind:value={instance.states[state].stroke_size}
					class="px-0.5 w-14 text-neutral-300 bg-neutral-600 border border-neutral-500 rounded-lg"
					id="editor-outline"
				/>
			</div>
			<div class="flex flex-row items-center">
				<label for="editor-font" class="mr-2">{$t("instance_editor.font")}</label>
				<input
					list="families"
					bind:value={instance.states[state].family}
					placeholder={$t("instance_editor.font.placeholder")}
					class="w-full px-1 text-neutral-300 bg-neutral-600 border border-neutral-500 rounded-lg"
					id="editor-font"
				/>
				<datalist id="families">
					<option value="Liberation Sans">Liberation Sans</option>
					<option value="Archivo Black">Archivo Black</option>
					<option value="Comic Neue">Comic Neue</option>
					<option value="Courier Prime">Courier Prime</option>
					<option value="Tinos">Tinos</option>
					<option value="Anton">Anton</option>
					<option value="Liberation Serif">Liberation Serif</option>
					<option value="Open Sans">Open Sans</option>
					<option value="Fira Sans">Fira Sans</option>
					<option disabled>──────────</option>
					{#each fonts as font}
						<option value={font}>{font}</option>
					{/each}
				</datalist>
			</div>
			<div class="flex flex-row items-center">
				<label for="editor-bold" class="mr-3 font-bold">B</label>
				<input
					type="checkbox"
					bind:checked={bold}
					on:change={() => (instance.states[state].style = bold && italic ? "Bold Italic" : bold ? "Bold" : italic ? "Italic" : "Regular")}
					class="mr-4 mt-1 scale-125"
					id="editor-bold"
				/>
				<label for="editor-italic" class="mr-3 italic">I</label>
				<input
					type="checkbox"
					bind:checked={italic}
					on:change={() => (instance.states[state].style = bold && italic ? "Bold Italic" : bold ? "Bold" : italic ? "Italic" : "Regular")}
					class="mr-4 mt-1 scale-125"
					id="editor-italic"
				/>
				<label for="editor-underline" class="mr-3 underline">U</label>
				<input type="checkbox" bind:checked={instance.states[state].underline} class="mr-4 mt-1 scale-125" id="editor-underline" />
				<label for="editor-size" class="mr-2">{$t("instance_editor.font.size")}</label>
				<input
					type="number"
					bind:value={instance.states[state].size}
					class="px-0.5 w-14 text-neutral-300 bg-neutral-600 border border-neutral-500 rounded-lg"
					id="editor-size"
				/>
			</div>
		</div>
	</div>
</div>
