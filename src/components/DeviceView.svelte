<script lang="ts">
	import type { ActionInstance } from "$lib/ActionInstance";
	import type { Context } from "$lib/Context";
	import type { DeviceInfo } from "$lib/DeviceInfo";
	import type { Profile } from "$lib/Profile";
	import type { CopiedItem } from "$lib/propertyInspector";

	import Key from "./Key.svelte";

	import { t } from "$lib/i18n";
	import { inspectedInstance, inspectedParentAction } from "$lib/propertyInspector";

	import { ask } from "@tauri-apps/plugin-dialog";
	import { invoke } from "@tauri-apps/api/core";

	export let device: DeviceInfo;
	export let profile: Profile;

	export let selectedDevice: string;
	export let press = false;

	function handleDragStart({ dataTransfer }: DragEvent, controller: string, position: number) {
		if (!dataTransfer) return;
		dataTransfer.effectAllowed = "move";
		dataTransfer.setData("controller", controller);
		dataTransfer.setData("position", position.toString());
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		if (!event.dataTransfer) return;
		if (event.dataTransfer.types.includes("action")) event.dataTransfer.dropEffect = "copy";
		else if (event.dataTransfer.types.includes("controller")) event.dataTransfer.dropEffect = "move";
	}

	async function handleDrop({ dataTransfer }: DragEvent, controller: string, position: number) {
		let context = { device: device.id, profile: profile.id, controller, position, page: device.current_page };
		let array = controller == "Encoder" ? profile.sliders : controller == "Infobar" ? profile.infobars : profile.keys;
		if (dataTransfer?.getData("action")) {
			let action = JSON.parse(dataTransfer?.getData("action"));
			if (array[position]) {
				return;
			}
			array[position] = await invoke("create_instance", { context, action });
			profile = profile;
		} else if (dataTransfer?.getData("controller")) {
			let oldController = dataTransfer?.getData("controller");
			let oldArray = oldController == "Encoder" ? profile.sliders : oldController == "Infobar" ? profile.infobars : profile.keys;
			let oldPosition = parseInt(dataTransfer?.getData("position"));
			let response: ActionInstance = await invoke("move_instance", {
				source: { device: device.id, profile: profile.id, controller: oldController, position: oldPosition, page: device.current_page },
				destination: context,
				retain: false,
			});
			if (response) {
				array[position] = response;
				oldArray[oldPosition] = null;
				profile = profile;
			}
		}
	}

	async function handlePaste(item: CopiedItem, destination: Context) {
		let array = destination.controller == "Encoder" ? profile.sliders : destination.controller == "Infobar" ? profile.infobars : profile.keys;

		if (item.type == "action") {
			if (array[destination.position]) return;
			array[destination.position] = await invoke("create_instance", { context: destination, action: item.action });
			profile = profile;
			return;
		}

		let response: ActionInstance = await invoke("move_instance", { source: item.source, destination, retain: true });
		if (response) {
			array[destination.position] = response;
			profile = profile;
		}
	}

	$: overflowsX = Math.max(device.columns, device.encoders, device.touchpoints) > 8;
	$: overflowsY = device.rows + Math.min(device.encoders, 1) + Math.min(device.touchpoints, 1) > 4;

	// Grid navigation: track focused cell and compute row lengths for arrow key movement.
	let focusedRow = 0;
	let focusedCol = 0;

	$: gridRowLengths = [
		...Array(device.rows).fill(device.columns),
		...(device.encoders > 0 ? [device.encoders] : []),
		...(device.touchpoints > 0 || device.infobars > 0 ? [device.touchpoints + device.infobars] : []),
	];
	$: encoderRowIndex = device.rows;
	$: touchpointRowIndex = device.rows + (device.encoders > 0 ? 1 : 0);
	$: keypadRowWidth = device.columns * 132;

	function flatIndexFromRowCol(row: number, col: number): number {
		let index = 0;
		for (let r = 0; r < row; r++) index += gridRowLengths[r];
		return index + col;
	}

	function rowColFromFlatIndex(flatIndex: number): [number, number] {
		let remaining = flatIndex;
		for (let r = 0; r < gridRowLengths.length; r++) {
			if (remaining < gridRowLengths[r]) return [r, remaining];
			remaining -= gridRowLengths[r];
		}
		return [0, 0];
	}

	function handleGridKeydown(event: KeyboardEvent) {
		const target = event.target as HTMLElement;
		if (target.getAttribute("role") !== "gridcell") return;
		if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;

		event.preventDefault();
		event.stopPropagation();

		let newRow = focusedRow;
		let newCol = focusedCol;

		switch (event.key) {
			case "ArrowRight":
				newCol = Math.min(focusedCol + 1, gridRowLengths[focusedRow] - 1);
				break;
			case "ArrowLeft":
				newCol = Math.max(focusedCol - 1, 0);
				break;
			case "ArrowDown":
				newRow = Math.min(focusedRow + 1, gridRowLengths.length - 1);
				newCol = Math.min(focusedCol, gridRowLengths[newRow] - 1);
				break;
			case "ArrowUp":
				newRow = Math.max(focusedRow - 1, 0);
				newCol = Math.min(focusedCol, gridRowLengths[newRow] - 1);
				break;
			case "Home":
				newCol = 0;
				break;
			case "End":
				newCol = gridRowLengths[focusedRow] - 1;
				break;
		}

		if (newRow === focusedRow && newCol === focusedCol) return;

		focusedRow = newRow;
		focusedCol = newCol;

		const grid = event.currentTarget as HTMLElement;
		const cells = grid.querySelectorAll("[role='gridcell']");
		(cells[flatIndexFromRowCol(newRow, newCol)] as HTMLElement)?.focus();
	}

	function handleGridFocusin(event: FocusEvent) {
		const grid = event.currentTarget as HTMLElement;
		const cells = Array.from(grid.querySelectorAll("[role='gridcell']"));
		const index = cells.indexOf(event.target as Element);
		if (index === -1) return;
		[focusedRow, focusedCol] = rowColFromFlatIndex(index);
	}

	// Pages strip: switch pages, add a new page, right-click a page chip to remove it.
	$: currentPage = Math.min(device.current_page ?? 0, Math.max(profile.pages - 1, 0));

	async function switchToPage(page: number) {
		if (page === currentPage) return;
		try {
			await invoke("set_selected_page", { device: device.id, page });
		} catch (error: any) {
			console.error(error);
		}
	}

	async function addPage() {
		try {
			await invoke("add_page", { device: device.id });
		} catch (error: any) {
			console.error(error);
		}
	}

	async function removePage(event: MouseEvent, page: number) {
		event.preventDefault();
		if (profile.pages <= 1) return;
		if (!(await ask($t("device_view.pages.prompt", { page: page + 1 }), { title: $t("device_view.pages.title"), okLabel: $t("dialog.yes"), cancelLabel: $t("dialog.no") }))) return;
		try {
			await invoke("remove_page", { device: device.id, page });
		} catch (error: any) {
			console.error(error);
		}
	}
</script>

{#key device}
	<span id="grid-description" class="sr-only">{$t("device_view.grid_description")}</span>
	<div
		class="flex flex-col justify-center grow px-16 py-6 overflow-auto"
		class:items-center={device.columns <= 9}
		class:hidden={$inspectedParentAction || selectedDevice != device.id}
		class:device-fade-x={overflowsX && !overflowsY}
		class:device-fade-y={overflowsY && !overflowsX}
		class:device-fade-xy={overflowsX && overflowsY}
		role="grid"
		aria-label={device.name}
		aria-describedby="grid-description"
		tabindex="-1"
		on:click={() => inspectedInstance.set(null)}
		on:keyup={() => inspectedInstance.set(null)}
		on:keydown|capture={handleGridKeydown}
		on:focusin={handleGridFocusin}
	>
		<div class="flex flex-col" role="rowgroup">
			{#each { length: device.rows } as _, r}
				<div class="flex flex-row" role="row">
					{#each { length: device.columns } as _, c}
						<Key
							context={{ device: device.id, profile: profile.id, controller: "Keypad", position: r * device.columns + c, page: device.current_page }}
							bind:inslot={profile.keys[r * device.columns + c]}
							on:dragover={handleDragOver}
							on:drop={(event) => handleDrop(event, "Keypad", r * device.columns + c)}
							on:dragstart={(event) => handleDragStart(event, "Keypad", r * device.columns + c)}
							{handlePaste}
							size={device.id.startsWith("sd-") && device.rows == 4 && device.columns == 8 ? 192 : 144}
							label="{$t('device_view.key')} {String.fromCharCode(65 + r)}{c + 1}"
							tabindex={focusedRow === r && focusedCol === c ? 0 : -1}
							{press}
						/>
					{/each}
				</div>
			{/each}
		</div>

		<div class="flex flex-row justify-between" role="row" style={`width: ${keypadRowWidth}px;`}>
			{#each { length: device.encoders } as _, i}
				<Key
					context={{ device: device.id, profile: profile.id, controller: "Encoder", position: i, page: device.current_page }}
					bind:inslot={profile.sliders[i]}
					on:dragover={handleDragOver}
					on:drop={(event) => handleDrop(event, "Encoder", i)}
					on:dragstart={(event) => handleDragStart(event, "Encoder", i)}
					{handlePaste}
					size={device.id.startsWith("sd-") && device.rows == 4 && device.columns == 8 ? 192 : 144}
					label="{$t('device_view.encoder')} {i + 1}"
					tabindex={focusedRow === encoderRowIndex && focusedCol === i ? 0 : -1}
				/>
			{/each}
		</div>

		<div class="flex flex-row items-center" role="row">
			{#each { length: device.touchpoints } as _, i}
				<!-- On the Stream Deck Neo, the infobar display sits physically between the two touchpoints. -->
				{#if device.infobars > 0 && i === 1}
					{#each { length: device.infobars } as _, j}
						<div class="px-3.5 py-[3.5px]">
							<Key
								context={{ device: device.id, profile: profile.id, controller: "Infobar", position: j, page: device.current_page }}
								bind:inslot={profile.infobars[j]}
								on:dragover={handleDragOver}
								on:drop={(event) => handleDrop(event, "Infobar", j)}
								on:dragstart={(event) => handleDragStart(event, "Infobar", j)}
								{handlePaste}
								size={device.id.startsWith("sd-") && device.rows == 4 && device.columns == 8 ? 192 : 144}
								width={248}
								height={58}
							/>
						</div>
					{/each}
				{/if}
				<Key
					context={{ device: device.id, profile: profile.id, controller: "Keypad", position: device.rows * device.columns + i, page: device.current_page }}
					bind:inslot={profile.keys[device.rows * device.columns + i]}
					on:dragover={handleDragOver}
					on:drop={(event) => handleDrop(event, "Keypad", device.rows * device.columns + i)}
					on:dragstart={(event) => handleDragStart(event, "Keypad", device.rows * device.columns + i)}
					{handlePaste}
					size={device.id.startsWith("sd-") && device.rows == 4 && device.columns == 8 ? 192 : 144}
					isTouchPoint
					label="{$t('device_view.touchpoint')} {i + 1}"
					tabindex={focusedRow === touchpointRowIndex && focusedCol === i ? 0 : -1}
				/>
			{/each}
		</div>
	</div>

	<div
		class="flex flex-row justify-center items-center gap-1.5 pb-3"
		class:hidden={$inspectedParentAction || selectedDevice != device.id}
		role="group"
		aria-label={$t("device_view.page", { page: currentPage + 1 })}
	>
		{#each Array(profile.pages) as _, i}
			<button
				type="button"
				aria-label={$t("device_view.page", { page: i + 1 })}
				aria-pressed={currentPage === i}
				class="flex justify-center items-center w-7 h-7 rounded-md text-sm font-medium transition-colors select-none {currentPage === i
					? "bg-neutral-200 text-neutral-900"
					: "bg-neutral-800 text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200"}"
				on:click={() => switchToPage(i)}
				on:contextmenu={(event) => removePage(event, i)}
			>
				{i + 1}
			</button>
		{/each}
		<button
			type="button"
			aria-label={$t("device_view.pages.add")}
			title={$t("device_view.pages.add")}
			class="flex justify-center items-center w-7 h-7 rounded-md text-lg leading-none bg-neutral-800 text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200 transition-colors select-none"
			on:click={addPage}
		>
			+
		</button>
	</div>
{/key}

<style>
	.device-fade-x {
		mask-image: linear-gradient(to right, transparent, black 7.5rem, black calc(100% - 7.5rem), transparent);
	}
	.device-fade-y {
		mask-image: linear-gradient(to bottom, transparent, black 7.5rem, black calc(100% - 7.5rem), transparent);
	}
	.device-fade-xy {
		mask-image:
			linear-gradient(to right, transparent, black 7.5rem, black calc(100% - 7.5rem), transparent),
			linear-gradient(to bottom, transparent, black 7.5rem, black calc(100% - 7.5rem), transparent);
		mask-composite: intersect;
	}
</style>
