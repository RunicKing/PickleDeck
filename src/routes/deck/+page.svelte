<script lang="ts">
	import type { DeviceInfo } from "$lib/DeviceInfo";
	import type { Profile } from "$lib/Profile";

	import DeviceView from "../../components/DeviceView.svelte";
	import { t } from "$lib/i18n";

	import { invoke } from "@tauri-apps/api/core";
	import { listen } from "@tauri-apps/api/event";
	import { onMount } from "svelte";

	const VIRTUAL_DEVICE_ID = "vd-VIRTUAL0";

	let devices: { [id: string]: DeviceInfo } = {};
	let device: DeviceInfo | null = null;
	let profile: Profile | null = null;
	let profiles: string[] = [];

	async function reload() {
		devices = await invoke("get_devices");
		device = devices[VIRTUAL_DEVICE_ID] ?? null;
		if (!device) {
			profile = null;
			profiles = [];
			return;
		}
		profiles = await invoke("get_profiles", { device: device.id });
		profile = await invoke("get_selected_profile", { device: device.id });
	}

	async function changeProfile(profileId: string) {
		if (!device) return;
		await invoke("set_selected_profile", { device: device.id, id: profileId });
		await reload();
	}

	onMount(() => {
		const unlisteners: Array<Promise<() => void>> = [
			listen("devices", () => reload()),
			listen(
				"switch_profile",
				async ({ payload }: { payload: { device: string; profile: string } }) => {
					if (payload.device == VIRTUAL_DEVICE_ID) await reload();
				},
			),
		];
		reload();
		return () => unlisteners.forEach((promise) => promise.then((fn) => fn()));
	});
</script>

<div class="flex flex-col items-center min-h-screen bg-neutral-950 select-none">
	<header class="flex flex-row items-center space-x-2 p-2 text-neutral-300">
		<span class="font-semibold text-lg pl-1">{$t("deck.title")}</span>
		{#if device && profile}
			<select
				value={profile.id}
				aria-label={$t("deck.profile")}
				class="text-sm px-1 py-0.5 bg-neutral-900 border border-neutral-700 rounded"
				on:change={(event) => changeProfile(event.currentTarget.value)}
			>
				{#each profiles as p}
					<option value={p}>{p}</option>
				{/each}
			</select>
		{/if}
		{#if !device}
			<span class="text-neutral-500 text-sm">{$t("deck.no_device")}</span>
		{/if}
	</header>
	{#if device && profile}
		<DeviceView {device} {profile} selectedDevice={device.id} press />
	{/if}
</div>
