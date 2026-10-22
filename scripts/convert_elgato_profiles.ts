#!/usr/bin/env -S deno run --allow-read --allow-write --allow-env
// PickleDeck M2: convert Elgato Stream Deck app ProfilesV3 bundles into OpenDeck
// profile JSON files. Pass 1: Keypad actions only, no custom per-profile images,
// no application auto-switch mapping.

import { join } from "https://deno.land/std@0.224.0/path/mod.ts";

const APPDATA = Deno.env.get("APPDATA")!;

interface Args {
  input: string;
  serial: string;
  deviceId: string;
  pluginsDir: string;
  out: string;
  report: string;
  dryRun: boolean;
}
const args: Args = {
  input: join(APPDATA, "Elgato", "StreamDeck", "ProfilesV3"),
  serial: "A7BSA4321ADW0S",
  deviceId: "sd-A7BSA4321ADW0S",
  pluginsDir: join(APPDATA, "opendeck", "plugins"),
  out: join(APPDATA, "opendeck", "profiles", "sd-A7BSA4321ADW0S"),
  report: "elgato-import-report.json",
  dryRun: Deno.args.includes("--dry-run"),
};

// Device geometry: Elgato model id -> keypad size. 20GBJ9901 = Stream Deck XL (8x4).
const KEY_COUNTS: Record<string, number> = { "20GBJ9901": 32 };
const GRID_WIDTH: Record<string, number> = { "20GBJ9901": 8 };

// ---------- action registry from installed OpenDeck plugins ----------
interface RegAction {
  plugin: string;
  uuid: string;
  name: string;
  icon: string;
}
interface Registry {
  byUuid: Map<string, RegAction>;
  plugins: Set<string>;
}
async function buildRegistry(dir: string): Promise<Registry> {
  const byUuid = new Map<string, RegAction>();
  const plugins = new Set<string>();
  for await (const entry of Deno.readDir(dir)) {
    if (!entry.isDirectory || !entry.name.endsWith(".sdPlugin")) continue;
    const manifestPath = join(dir, entry.name, "manifest.json");
    let manifest: Record<string, unknown>;
    try {
      manifest = JSON.parse(await Deno.readTextFile(manifestPath));
    } catch {
      continue;
    }
    const pluginId = String(manifest.UUID ?? entry.name.replace(".sdPlugin", ""));
    plugins.add(pluginId);
    for (const action of (manifest.Actions as unknown[]) ?? []) {
      const a = action as Record<string, unknown>;
      const uuid = String(a.UUID ?? "");
      if (!uuid) continue;
      const states = (a.States as Record<string, unknown>[] | undefined) ?? [];
      byUuid.set(uuid, {
        plugin: pluginId,
        uuid,
        name: String(a.Name ?? ""),
        icon: String(states[0]?.Image ?? a.Icon ?? ""),
      });
    }
  }
  return { byUuid, plugins };
}

// ---------- Elgato -> OpenDeck action-UUID special mappings ----------
const BUILTIN_MULTIACTION = "opendeck.multiaction";
const BUILTIN_TOGGLEACTION = "opendeck.toggleaction";
const SP_INPUTSIM = "com.amansprojects.starterpack.inputsimulation";
const SP_PLUGIN = "com.amansprojects.starterpack";
const SP_SWITCHPROFILE = "com.amansprojects.starterpack.switchprofile";

const ELGATO_MULTIACTION = ["com.elgato.streamdeck.multiaction"];
const ELGATO_TOGGLE = ["com.elgato.streamdeck.toggle", "com.elgato.streamdeck.toggleaction"];

function vkName(code: number): string | null {
  const specials: Record<number, string> = {
    8: "backspace", 9: "tab", 13: "enter", 20: "capslock", 27: "esc", 32: "space",
    33: "pagedown", 34: "pageup", 35: "end", 36: "home", 37: "left", 38: "up",
    39: "right", 40: "down", 45: "insert", 46: "delete", 91: "win", 93: "menu",
    112: "f1", 113: "f2", 114: "f3", 115: "f4", 116: "f5", 117: "f6", 118: "f7",
    119: "f8", 120: "f9", 121: "f10", 122: "f11", 123: "f12", 144: "numlock",
    186: ";", 187: "=", 188: ",", 189: "-", 190: ".", 191: "/", 192: "`",
    219: "[", 220: "\\", 221: "]", 222: "'",
  };
  if (specials[code]) return specials[code];
  if (code >= 48 && code <= 57) return String.fromCharCode(code);
  if (code >= 65 && code <= 90) return String.fromCharCode(code + 32);
  if (code >= 96 && code <= 105) return "num" + (code - 96);
  if (code >= 106 && code <= 111) return ["num*", "num+", "num-", "numseparator", "num/"][code - 106];
  return null;
}
function convertHotkey(settings: Record<string, unknown>): Record<string, unknown> | null {
  const combos = settings["Hotkeys"];
  if (!Array.isArray(combos) || combos.length === 0) return null;
  for (const combo of combos) {
    const c = combo as Record<string, unknown>;
    const vk = Number(c.VKeyCode ?? -1);
    const nc = Number(c.NativeCode ?? 0);
    if (vk < 0 && nc === 146) continue; // empty filler entry appended by the Elgato editor
    const code = vk >= 0 ? vk : nc;
    const mods: string[] = [];
    if (c.KeyCtrl) mods.push("control");
    if (c.KeyShift) mods.push("shift");
    if (c.KeyOption) mods.push("alt");
    if (c.KeyCmd) mods.push("win");
    const keyPart = vkName(code) ?? `VK${code}`;
    if (!keyPart && mods.length === 0) continue;
    return { down: [...mods, keyPart].filter(Boolean).join("+"), up: "" };
  }
  return null;
}

// ---------- conversion core ----------
const report = {
  groups: [] as unknown[],
  unmapped: {} as Record<string, number>,
  mappedByUuid: {} as Record<string, number>,
  mapped: 0,
  slots: 0,
  placed: 0,
  imagesSkipped: 0,
  hotkeys: 0,
  switchProfiles: 0,
  pageNav: 0,
  multiChildren: 0,
};
function noteUnmapped(uuid: string) {
  report.unmapped[uuid] = (report.unmapped[uuid] ?? 0) + 1;
}
function noteMapped(uuid: string) {
  report.mapped++;
  report.mappedByUuid[uuid] = (report.mappedByUuid[uuid] ?? 0) + 1;
}

interface ElgState {
  Image?: unknown; Title?: unknown; TitleColor?: unknown; ShowTitle?: unknown;
  TitleAlignment?: unknown; FontFamily?: unknown; FontStyle?: unknown;
  FontSize?: unknown; FontUnderline?: unknown;
}
interface ElgAction {
  UUID: string; Name?: string; Settings?: Record<string, unknown>;
  State?: number; States?: ElgState[];
}
type OpenDeckState = Record<string, string | number | boolean>;
interface OpenDeckInstance {
  action: { name: string; uuid: string; plugin: string; icon: string; states: OpenDeckState[] };
  context: string;
  states: OpenDeckState[];
  current_state: number;
  settings: Record<string, unknown>;
  children: OpenDeckInstance[] | null;
}

function mapStates(elgStates: ElgState[] | undefined): OpenDeckState[] {
  const out: OpenDeckState[] = [];
  for (const s of elgStates ?? []) {
    const st: OpenDeckState = {};
    if (typeof s.Image === "string" && s.Image) report.imagesSkipped++;
    if (typeof s.Title === "string") st.Title = s.Title;
    if (typeof s.TitleColor === "string") st.TitleColor = s.TitleColor;
    if (typeof s.ShowTitle === "boolean") st.ShowTitle = s.ShowTitle;
    if (typeof s.TitleAlignment === "string") st.TitleAlignment = s.TitleAlignment;
    if (typeof s.FontFamily === "string") st.FontFamily = s.FontFamily;
    if (typeof s.FontStyle === "string") st.FontStyle = s.FontStyle;
    if (typeof s.FontSize === "number") st.FontSize = s.FontSize;
    if (typeof s.FontUnderline === "boolean") st.FontUnderline = s.FontUnderline;
    out.push(st);
  }
  return out;
}
interface Ctx {
  registry: Registry;
  device: string;
  profileName: string;
  pageUuid: string;
  pageNames: Map<string, string>; // lowercase pageuuid -> profile name
  parentMap: Map<string, string>; // lowercase pageuuid -> parent profile name ("" = ambiguous)
}

function switchInst(ctx: Ctx, src: ElgAction, uuid: string, name: string, spSettings: Record<string, unknown>): OpenDeckInstance {
  const states = mapStates(src.States);
  return {
    action: { name, uuid: SP_SWITCHPROFILE, plugin: SP_PLUGIN, icon: "icons/plugin", states: [] },
    context: "", states, current_state: Number(src.State ?? 0), settings: spSettings, children: null,
  };
}

function convertInstance(src: ElgAction, uuid: string, ctx: Ctx, position: number, index: number): OpenDeckInstance | null {
  const settings = (src.Settings ?? {}) as Record<string, unknown>;
  let instance: OpenDeckInstance | null = null;

  if (ELGATO_MULTIACTION.includes(uuid)) {
    const childrenRaw = (settings["Actions"] as Record<string, unknown>[]) ?? [];
    const children: OpenDeckInstance[] = [];
    childrenRaw.forEach((child, i) => {
      const cu = String(child["UUID"] ?? "");
      const c = convertInstance(child as ElgAction, cu, ctx, position, i);
      if (c) { children.push(c); report.multiChildren++; }
      else noteUnmapped("multiaction-child:" + cu);
    });
    console.log(`      multi@${position}: ${children.length}/${childrenRaw.length} children ok`);
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name ?? "Multi Action", uuid: BUILTIN_MULTIACTION, plugin: "opendeck", icon: "opendeck/multi-action.png", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: {}, children: children.length ? children : null,
    };
  } else if (
    uuid === "com.elgato.streamdeck.multiactions.routine" ||
    ELGATO_MULTIACTION.includes(uuid)
  ) {
    // Routines store their steps in an `Actions` batch tree (v3 profile format);
    // classic multi-actions store them in Settings.Actions. Both flatten into
    // opendeck.multiaction children.
    const steps: ElgAction[] = [];
    const walk = (node: unknown) => {
      if (Array.isArray(node)) { node.forEach(walk); return; }
      if (node && typeof node === "object") {
        const o = node as Record<string, unknown>;
        if (typeof o.UUID === "string" && o.UUID !== "com.elgato.streamdeck.multiactions.routine") steps.push(o as ElgAction);
        if (o.Actions !== undefined) walk(o.Actions);
      }
    };
    if (uuid === "com.elgato.streamdeck.multiactions.routine") walk((src as Record<string, unknown>).Actions);
    else walk(settings["Actions"]);
    const children: OpenDeckInstance[] = [];
    steps.forEach((step, i) => {
      const cu = String(step.UUID ?? "");
      const c = convertInstance(step, cu, ctx, position, i);
      if (c) { children.push(c); report.multiChildren++; }
      else noteUnmapped("routine-child:" + cu);
    });
    console.log(`      ${ELGATO_MULTIACTION.includes(uuid) ? "multi" : "routine"}@${position}: ${children.length}/${steps.length} children ok`);
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name ?? "Multi Action", uuid: BUILTIN_MULTIACTION, plugin: "opendeck", icon: "opendeck/multi-action.png", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: {}, children: children.length ? children : null,
    };
  } else if (uuid === "com.elgato.streamdeck.system.open") {
    const file = String(settings["path"] ?? "");
    if (!file) { noteUnmapped("system.open:no-path"); return null; }
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name || "Open File", uuid: "com.amansprojects.starterpack.runcommand", plugin: SP_PLUGIN, icon: "icons/plugin", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: { file }, children: null,
    };
  } else if (uuid === "com.elgato.streamdeck.system.website") {
    const url = String(settings["path"] ?? "");
    if (!url) { noteUnmapped("system.website:no-path"); return null; }
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name || "Open Website", uuid: "com.amansprojects.starterpack.openurl", plugin: SP_PLUGIN, icon: "icons/plugin", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: { down: url, up: "" }, children: null,
    };
  } else if (uuid === "com.elgato.streamdeck.system.text") {
    const text = settings["pastedText"];
    if (typeof text !== "string" || !text) { noteUnmapped("system.text:empty"); return null; }
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name || "Type Text", uuid: SP_INPUTSIM, plugin: SP_PLUGIN, icon: "icons/plugin", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: { down: text, up: "" }, children: null,
    };
  } else if (ELGATO_TOGGLE.includes(uuid)) {
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name ?? "Toggle Action", uuid: BUILTIN_TOGGLEACTION, plugin: "opendeck", icon: "opendeck/toggle-action.png", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: {}, children: null,
    };
  } else if (uuid === "com.elgato.streamdeck.system.hotkey" || uuid === "com.elgato.streamdeck.hotkey") {
    report.hotkeys++;
    const conv = convertHotkey(settings);
    if (!conv) { noteUnmapped("hotkey:" + JSON.stringify(settings).slice(0, 80)); return null; }
    const states = mapStates(src.States);
    instance = {
      action: { name: src.Name || "Hotkey", uuid: SP_INPUTSIM, plugin: SP_PLUGIN, icon: "icons/plugin", states: [] },
      context: "", states, current_state: Number(src.State ?? 0), settings: conv, children: null,
    };
  } else if (
    uuid === "com.elgato.streamdeck.profile.openchild" ||
    uuid === "com.elgato.streamdeck.profile.rotate" ||
    uuid === "com.elgato.streamdeck.page.goto"
  ) {
    if (uuid === "com.elgato.streamdeck.profile.rotate") {
      const dev = String(settings["DeviceUUID"] ?? "");
      if (dev.toUpperCase().startsWith("UISD")) {
        noteUnmapped("rotate-cross-device(UISD)");
        return null;
      }
    }
    report.pageNav++;
    const target = String(settings["ProfileUUID"] ?? "").toLowerCase();
    const targetProfile = ctx.pageNames.get(target);
    if (!targetProfile) { noteUnmapped(uuid + ":target-not-physical"); return null; }
    instance = switchInst(ctx, src, uuid, src.Name || "Open Page", { profile: targetProfile });
  } else if (uuid === "com.elgato.streamdeck.profile.backtoparent") {
    // Elgato pops a navigation stack; starterpack can only jump statically.
    // Resolve via openchild edges (child -> parent); ambiguous children fail.
    const parent = ctx.parentMap.get(ctx.pageUuid);
    if (!parent) { noteUnmapped("backtoparent(no-parent-found)"); return null; }
    report.pageNav++;
    report.mappedByUuid["backtoparent->switchprofile"] = (report.mappedByUuid["backtoparent->switchprofile"] ?? 0) + 1;
    const pseudo: ElgAction = src;
    instance = switchInst(ctx, pseudo, uuid, src.Name || "Back to Parent", { profile: parent });
  } else if (uuid === "com.elgato.streamdeck.switchprofile") {
    report.switchProfiles++;
    instance = switchInst(ctx, src, uuid, src.Name || "Switch Profile", { profile: String(settings["Name"] ?? "") });
  } else {
    const reg = ctx.registry.byUuid.get(uuid);
    if (reg && ctx.registry.plugins.has(reg.plugin)) {
      noteMapped(uuid);
      const states = mapStates(src.States);
      instance = {
        action: { name: (src.Name && src.Name.trim()) || reg.name, uuid: reg.uuid, plugin: reg.plugin, icon: reg.icon, states: [] },
        context: "", states, current_state: Number(src.State ?? 0), settings, children: null,
      };
    }
  }

  if (!instance) { noteUnmapped(uuid); return null; }
  instance.context = `${ctx.device}.${ctx.profileName}.Keypad.${position}.${index}`;
  return instance;
}

const usedNames = new Set<string>();
function uniqueName(base: string, groupId: string): string {
  let name = base;
  if (usedNames.has(name)) {
    name = `${base} (${groupId.slice(0, 8)})`;
    while (usedNames.has(name)) name += " x";
  }
  usedNames.add(name);
  return name;
}
const baseName = (group: string, n: number, pageName: string) =>
  (pageName && pageName.trim() ? `${group} - ${pageName.trim()}` : `${group} ${n}`)
    .replace(/[\\/:*?"<>|]/g, "_").slice(0, 70);

// ---------- main: two phases ----------
const registry = await buildRegistry(args.pluginsDir);
console.log(`registry: ${registry.plugins.size} plugins, ${registry.byUuid.size} actions`);

// Phase 1: collect matching groups and assign unique profile names to listed pages.
interface PageRef {
  group: string; modelName: string; groupPath: string; pageDir: string;
  ordinal: number; profileName: string; page: { Name: string; Controllers: { Type: string; Actions: Record<string, ElgAction> }[] };
}
const pages: PageRef[] = [];

for await (const groupDir of Deno.readDir(args.input)) {
  if (!groupDir.isDirectory || !groupDir.name.endsWith(".sdProfile")) continue;
  const gpath = join(args.input, groupDir.name);
  let gmanifest: { Name: string; Device: { Model: string; UUID: string }; Pages: { Pages: string[] } };
  try {
    gmanifest = JSON.parse(await Deno.readTextFile(join(gpath, "manifest.json")));
  } catch {
    continue;
  }
  if (!gmanifest.Device.UUID.includes(args.serial)) continue;
  const model = gmanifest.Device.Model;
  const keyCount = KEY_COUNTS[model];
  if (!keyCount) { noteUnmapped(`skipped-group-model:${model}`); continue; }
  const pageIdToOrdinal = new Map<string, number>();
  gmanifest.Pages.Pages.forEach((id, i) => pageIdToOrdinal.set(String(id).toLowerCase(), i + 1));

  for await (const pageDir of Deno.readDir(join(gpath, "Profiles"))) {
    if (!pageDir.isDirectory) continue;
    const pid = pageDir.name.toLowerCase();
    // Listed pages get "group n" names; unlisted dirs are real child pages that
    // openchild/rotate actions navigate to, so they convert too (ordinal 0).
    const ordinal = pageIdToOrdinal.get(pid) ?? 0;
    const pageManifestPath = join(gpath, "Profiles", pageDir.name, "manifest.json");
    const page: PageRef["page"] = JSON.parse(await Deno.readTextFile(pageManifestPath));
    pages.push({
      group: gmanifest.Name, modelName: model, groupPath: gpath, pageDir: pageDir.name,
      ordinal, profileName: "", page,
    });
  }
}
// assign names
for (const p of pages) {
  const guid = p.groupPath.split("\\").pop()!.replace(".sdProfile", "");
  let base: string;
  if (p.ordinal > 0) {
    base = baseName(p.group, p.ordinal, p.page.Name ?? "");
  } else if (p.page.Name && p.page.Name.trim()) {
    base = `${p.group} - ${p.page.Name.trim()}`.replace(/[\\/:*?"<>|]/g, "_");
  } else {
    base = `${p.group} child ${p.pageDir.slice(0, 8).toLowerCase()}`;
  }
  p.profileName = uniqueName(base.slice(0, 70), guid);
}
const pageNames = new Map<string, string>();
for (const p of pages) pageNames.set(p.pageDir.toLowerCase(), p.profileName);

// Build child->parent edges from openchild actions (backtoparent needs them).
const parentMap = new Map<string, string>();
for (const p of pages) {
  for (const controller of p.page.Controllers) {
    const actions = controller.Actions;
    if (!actions || typeof actions !== "object") continue;
    for (const action of Object.values(actions)) {
      if (action.UUID !== "com.elgato.streamdeck.profile.openchild") continue;
      const target = String((action.Settings ?? {})["ProfileUUID"] ?? "").toLowerCase();
      if (!target) continue;
      if (parentMap.has(target) && parentMap.get(target) !== p.profileName) parentMap.set(target, "");
      else if (!parentMap.has(target)) parentMap.set(target, p.profileName);
    }
  }
}

// Phase 2: convert
for (const p of pages) {
  const keyCount = KEY_COUNTS[p.modelName]!;
  const gridWidth = GRID_WIDTH[p.modelName]!;
  const ctx: Ctx = { registry, device: args.deviceId, profileName: p.profileName, pageUuid: p.pageDir.toLowerCase(), pageNames, parentMap };
  const keys: (OpenDeckInstance | null)[] = new Array(keyCount).fill(null);
  let used = 0, dropped = 0;
  for (const controller of p.page.Controllers) {
    if (controller.Type !== "Keypad") { noteUnmapped(`controller:${controller.Type}`); continue; }
    const actions = controller.Actions;
    if (!actions || typeof actions !== "object") continue;
    for (const [posKey, src] of Object.entries(actions)) {
      const [row, col] = posKey.split(",").map(Number);
      const position = row * gridWidth + col; // row-major, XL hardware order
      report.slots++;
      const inst = convertInstance(src, src.UUID, ctx, position, 0);
      if (inst) { keys[position] = inst; used++; report.placed++; } else dropped++;
    }
  }
  const profile = { id: p.profileName, keys, sliders: [], infobars: [] };
  if (!args.dryRun) {
    await Deno.mkdir(args.out, { recursive: true });
    await Deno.writeTextFile(join(args.out, `${p.profileName}.json`), JSON.stringify(profile, null, "\t"));
  }
  console.log(`  [${p.group}] ${p.profileName}: ${used} placed, ${dropped} dropped`);
}

await Deno.writeTextFile(args.report, JSON.stringify(report, null, "\t"));
console.log("\n=== REPORT ===");
console.log(`placed: ${report.placed}/${report.slots}; mapped(plugins): ${report.mapped}; hotkeys: ${report.hotkeys}; pageNav: ${report.pageNav}; multi-children: ${report.multiChildren}; images skipped: ${report.imagesSkipped}`);
console.log("mapped breakdown:", JSON.stringify(report.mappedByUuid, null, "\t"));
console.log("unmapped:", JSON.stringify(report.unmapped, null, "\t"));
