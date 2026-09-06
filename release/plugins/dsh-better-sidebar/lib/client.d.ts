import type { ReactNode } from "react";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type SettingType = "switch" | "text" | "number" | "select";

export interface SidebarSettingOption {
  value: string | number | boolean;
  title: string | (() => string);
  desc?: string | (() => string);
  icon?: ReactNode | ((size: number) => ReactNode);
}

export interface SidebarSettingToggle {
  key: string;
  title: string | (() => string);
  desc?: string | (() => string);
  type?: SettingType;
  min?: number;
  max?: number;
  placeholder?: string;
  unit?: string;
  options?: readonly SidebarSettingOption[];
  multi?: boolean;
  /** Rust-host extension. Upstream descriptors may omit this field. */
  defaultValue?: JsonValue;
}

export interface SidebarTab {
  id: string;
  type: string;
  title: string;
  path?: string;
  diff?: JsonValue;
  meta?: JsonValue;
}

export interface SidebarLeaf { kind: "leaf"; id: string; tabs: SidebarTab[]; active: string | null }
export interface SidebarSplit { kind: "split"; id: string; dir: "row" | "col"; sizes: number[]; children: SplitNode[] }
export type SplitNode = SidebarLeaf | SidebarSplit;
export interface FloatWindow { id: string; tab: SidebarTab; x: number; y: number; w: number; h: number }

export interface SidebarState {
  open: boolean;
  width: number;
  fullscreen: boolean;
  tab: string;
  dir: string;
  file: string;
  files: string[];
  url: string;
  webTabs: string[];
  history: string[];
  historyIndex: number;
  pluginTabs: SidebarTab[];
  activePane: string;
  splits: SplitNode;
  bottomOpen: boolean;
  bottomHeight: number;
  bottomSplits: SplitNode;
  floats: FloatWindow[];
  layoutSerial: number;
  [key: string]: unknown;
}

export interface SidebarPreferences {
  width: number;
  rememberWidth: boolean;
  fullscreenOnOpen: boolean;
  showFiles: boolean;
  showGit: boolean;
  showBrowser: boolean;
  showTerminal: boolean;
  httpLinks: "sidebar" | "external";
  httpsLinks: "sidebar" | "external";
  tabsEnabled: Record<string, boolean>;
  viewersEnabled: Record<string, boolean>;
  pluginSettings: Record<string, Record<string, JsonValue>>;
  sessionLayouts: Record<string, unknown>;
}

export interface SessionScope { sessionId: string; cwd?: string }
export interface SidebarSnapshot {
  sessionId?: string;
  state?: SidebarState;
  prefs: SidebarPreferences;
}

export interface SidebarStore {
  getSnapshot(): SidebarSnapshot;
  getPrefs(): SidebarPreferences;
  subscribe(listener: () => void): () => void;
  reduce(reducer: (state: SidebarState) => SidebarState): void;
}

export interface SidebarSettingsRenderProps {
  store: SidebarStore;
  service: BetterSidebarService;
  prefs: SidebarPreferences;
  pluginSettings: Record<string, JsonValue>;
  updatePluginSetting(key: string, value: JsonValue): void;
  close(): void;
}

export interface SidebarSettingsDeclaration {
  toggles?: readonly SidebarSettingToggle[];
  pluginToggles?: readonly SidebarSettingToggle[];
  render?: (props: SidebarSettingsRenderProps) => ReactNode;
}

export interface TabComponentProps {
  ctx: unknown;
  store: SidebarStore;
  scope: SessionScope;
  tab: SidebarTab;
  visible: boolean;
  /** Resolved pluginToggles values, including Rust-host defaultValue entries. */
  pluginSettings: Record<string, JsonValue>;
}

export interface TabDescriptor {
  id: string;
  title: string | (() => string);
  icon?: ReactNode | ((size: number) => ReactNode);
  order?: number;
  hidden?: boolean;
  /** False keeps safety-critical tabs mounted and removes every close control. */
  closable?: boolean;
  available?: (ctx: unknown, scope: SessionScope, state: SidebarState) => boolean;
  single?: boolean;
  dedupeKey?: (tab: SidebarTab) => string | undefined;
  createTab?: (state: SidebarState) => { tab: SidebarTab; patch?: Partial<SidebarState> } | null;
  urlTarget?: (url: URL) => boolean;
  settings?: SidebarSettingsDeclaration;
  badge?: (ctx: unknown, scope: SessionScope, state: SidebarState) => string | number | null | undefined;
  onOpen?: (tab: SidebarTab, scope: SessionScope) => void;
  onActivate?: (tab: SidebarTab, scope: SessionScope) => void;
  onClose?: (tab: SidebarTab, scope: SessionScope) => void;
  component: (props: TabComponentProps) => ReactNode;
}

export type FileFetchStrategy = "none" | "fsRead" | "mediaUrl" | "custom" | "binary-download";
export interface FileViewerProps {
  ctx: unknown;
  store: SidebarStore;
  scope: SessionScope;
  path: string;
  title: string;
  viewerId: string;
  content?: string;
  mediaUrl?: string;
  customData?: unknown;
  pluginSettings: Record<string, JsonValue>;
}

export interface FileViewerDescriptor {
  id: string;
  title?: string | (() => string);
  icon?: ReactNode | ((size: number) => ReactNode);
  exts: readonly string[];
  priority?: number;
  fetchStrategy: FileFetchStrategy;
  detect?: (path: string, head: Uint8Array) => boolean;
  load?: (path: string, scope: SessionScope, signal?: AbortSignal) => Promise<unknown>;
  settings?: SidebarSettingsDeclaration;
  component: (props: FileViewerProps) => ReactNode;
}

export interface OpenTabSeed {
  type: string;
  title?: string;
  path?: string;
  diff?: JsonValue;
  id?: string;
  url?: string;
  meta?: JsonValue;
}

export interface BetterSidebarService {
  registerTab(descriptor: TabDescriptor): () => void;
  registerFileViewer(descriptor: FileViewerDescriptor): () => void;
  getTabs(): readonly TabDescriptor[];
  getFileViewers(): readonly FileViewerDescriptor[];
  getTab(id: string): TabDescriptor | undefined;
  isTabEnabled(id: string): boolean;
  isViewerEnabled(id: string): boolean;
  matchFileViewer(path: string, head?: Uint8Array): FileViewerDescriptor | undefined;
  openTab(seed: OpenTabSeed, scope?: SessionScope): void;
  closeTab(tabId: string, scope?: SessionScope): void;
  activateTab(tabId: string, scope?: SessionScope): void;
  updateTab(tabId: string, patch: Pick<Partial<SidebarTab>, "title" | "path" | "meta">, scope?: SessionScope): void;
  openFile(scope: SessionScope, path: string, title?: string): void;
  moveTab(tabId: string, target: { kind: "right" | "bottom"; paneId?: string; edge?: "center" | "left" | "right" | "top" | "bottom" }, scope?: SessionScope): void;
  floatTab(tabId: string, geometry?: Partial<Pick<FloatWindow, "x" | "y" | "w" | "h">>, scope?: SessionScope): void;
  updateFloat(floatId: string, geometry: Partial<Pick<FloatWindow, "x" | "y" | "w" | "h">>, scope?: SessionScope): void;
  subscribe(listener: () => void): () => void;
  getSnapshot(): SidebarSnapshot;
  subscribeState(listener: () => void): () => void;
  readonly version: string;
  readonly features: readonly string[];
}

declare module "@deepseek-ai/cordis" {
  interface Context {
    readonly betterSidebar: BetterSidebarService;
  }
}

export const inject: readonly string[];
export const features: readonly string[];
export const settingsDescriptor: Readonly<Record<string, unknown>>;
export function apply(ctx: unknown): void;
