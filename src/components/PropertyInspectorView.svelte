<script lang="ts">
	import type { ActionInstance } from "$lib/ActionInstance";
	import type { DeviceInfo } from "$lib/DeviceInfo";
	import type { Profile } from "$lib/Profile";

	import { t } from "$lib/i18n";
	import { getWebserverUrl, getWebSocketPort } from "$lib/ports";
	import { inspectedInstance } from "$lib/propertyInspector";

	import { invoke } from "@tauri-apps/api/core";
	import { listen } from "@tauri-apps/api/event";

	let iframeContainer: HTMLDivElement;
	let iframeClosePopup: HTMLButtonElement;
	let iframePopupsOpen: string[] = [];

	export let device: DeviceInfo;
	export let profile: Profile;

	async function iframeOnLoad(event: Event, instance: ActionInstance) {
		const iframe = event.target;
		const split = instance.context.split(".");

		const position = parseInt(split[3]);
		let coordinates: { row: number; column: number };
		if (split[2] == "Encoder") {
			coordinates = { row: 0, column: position };
		} else {
			coordinates = { row: Math.floor(position / device.columns), column: position % device.columns };
		}

		if (instance == null || !iframe?.src || !iframe.src.startsWith(getWebserverUrl())) return;
		const info = JSON.stringify(await invoke("make_info", { plugin: instance.action.plugin }));

		iframe?.contentWindow?.postMessage(
			{
				event: "connect",
				payload: [
					getWebSocketPort(),
					instance.context,
					"registerPropertyInspector",
					info,
					JSON.stringify({
						action: instance.action.uuid,
						context: instance.context,
						device: split[0],
						payload: {
							settings: instance.settings,
							coordinates,
							controller: split[2],
							state: instance.current_state,
							isInMultiAction: parseInt(split[4]) != 0,
						},
					}),
				],
			},
			getWebserverUrl(),
		);
	}

	const closePopup = (context: string) => {
		const iframe = inspectedInst && inspectedInst.context == context ? inspectedIframe : null;
		if (iframe) {
			iframe.style.position = "";
			iframe.style.left = "";
			iframe.style.top = "";
			iframe.style.width = "100%";
			iframe.style.height = "100%";
			iframe.style.display = "block";
			iframe.contentWindow?.postMessage({ event: "windowClosed" }, getWebserverUrl());
		}

		iframePopupsOpen = iframePopupsOpen.filter((e) => e != context);

		if (iframePopupsOpen.length == 0) {
			iframeContainer.style.position = "";
			iframeContainer.style.width = "";
			iframeContainer.style.height = "";
			iframeContainer.style.padding = "";
			iframeContainer.style.zIndex = "0";
			iframeContainer.style.maxHeight = "";

			iframeClosePopup.style.display = "none";
		}
	};

	window.addEventListener("message", ({ data }) => {
		if (data.event == "windowOpened") {
			const iframe = inspectedIframe;
			iframe.style.position = "absolute";
			iframe.style.left = "36px";
			iframe.style.top = "36px";
			iframe.style.width = "calc(100% - 72px)";
			iframe.style.height = "calc(100% - 72px)";
			iframe.style.display = "block";

			iframePopupsOpen.push(data.payload);

			iframeContainer.style.position = "absolute";
			iframeContainer.style.width = "100%";
			iframeContainer.style.height = "100%";
			iframeContainer.style.padding = "36px";
			iframeContainer.style.zIndex = "20";
			iframeContainer.style.maxHeight = "100vh";

			iframeClosePopup.style.display = "block";
		} else if (data.event == "windowClosed") {
			closePopup(data.payload);
		} else if (data.event == "openUrl") {
			invoke("open_url", { url: data.payload });
		} else if (data.event == "fetch") {
			function combineUint8Arrays(arrays: Uint8Array[]): Uint8Array {
				const totalLength = arrays.reduce((acc, curr) => acc + curr.length, 0);
				let mergedArray = new Uint8Array(totalLength);
				let offset = 0;

				arrays.forEach((item) => {
					mergedArray.set(item, offset);
					offset += item.length;
				});

				return mergedArray;
			}

			window
				// @ts-expect-error
				.fetchCORS(...data.payload.args)
				.then(async (response: Response) => {
					const chunks = [];
					if (response.body) {
						const reader = response.body.getReader();
						while (true) {
							const { done, value } = await reader.read();
							if (done) break;
							chunks.push(value);
						}
					}
					const body = combineUint8Arrays(chunks);

					inspectedIframe?.contentWindow?.postMessage(
						{
							event: "fetchResponse",
							payload: {
								id: data.payload.id,
								response: {
									url: response.url,
									body,
									headers: response.headers.entries().toArray(),
									status: response.status,
									statusText: response.statusText,
								},
							},
						},
						getWebserverUrl(),
					);
				})
				.catch((error: any) => {
					inspectedIframe?.contentWindow?.postMessage({ event: "fetchError", payload: { id: data.payload.id, error } }, getWebserverUrl());
				});
		}
	});

	const nonNull = <T,>(o: T | null): o is T => o != null;
	$: instances = profile.keys
		.filter(nonNull)
		.reduce((prev, current) => prev.concat(current.children ? [current, ...current.children] : current), [] as ActionInstance[])
		.concat(profile.sliders.filter(nonNull))
		.concat(profile.infobars.filter(nonNull));

	// Only the inspected instance's property inspector is instantiated; loading every plugin's
	// PI at once let idle PI scripts (polling, reconnects) burn CPU inside the main renderer.
	let inspectedInst: ActionInstance | undefined;
	$: inspectedInst = instances.find((i) => i.context == $inspectedInstance);
	let inspectedIframe: HTMLIFrameElement;

	listen("plugin_reloaded", ({ payload }: { payload: string }) => {
		if (inspectedInst?.action.plugin == payload && inspectedIframe) {
			inspectedIframe.src += "";
		}
	});
</script>

<svelte:window
	on:keydown={(event) => {
		if (event.key == "Escape" && iframePopupsOpen.length > 0) {
			closePopup(iframePopupsOpen[iframePopupsOpen.length - 1]);
		}
	}}
/>

<div class="grow min-h-64 max-h-96 overflow-auto bg-neutral-800 border-t border-neutral-700" bind:this={iframeContainer}>
	<button
		bind:this={iframeClosePopup}
		on:click={() => closePopup(iframePopupsOpen[iframePopupsOpen.length - 1])}
		class="absolute top-2 right-2 text-2xl text-neutral-300 font-bold hidden"
	>
		✕
	</button>
	{#key $inspectedInstance}
		{#if inspectedInst?.action.property_inspector}
			<iframe
				title={$t("property_inspector.title")}
				class="w-full h-full block"
				src={getWebserverUrl(inspectedInst.action.property_inspector + "|opendeck_property_inspector")}
				name={inspectedInst.context}
				bind:this={inspectedIframe}
				on:load={(event) => iframeOnLoad(event, inspectedInst)}
			/>
		{/if}
	{/key}
</div>
