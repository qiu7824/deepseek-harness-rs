window.__ModuleLoader__.load({
	id: "@deepseek-ai/dsh-client-ui-settings",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		let _deepseek_ai_cordis = require("@deepseek-ai/cordis");
        const React=require("react");
        function SettingsSwitch({checked,onChange,label,id,disabled=false}) {
            return React.createElement("button",{type:"button",role:"switch",id,"aria-label":label,"aria-checked":checked,disabled,className:"dshSettingsSwitch",onClick:()=>onChange(!checked)},React.createElement("span",{className:"dshSettingsSwitchThumb"}));
        }

		let _deepseek_ai_dsh_client_schema_form = require("@deepseek-ai/dsh-client-schema-form");
		let _deepseek_ai_dsh_client_runtime_client = require("@deepseek-ai/dsh-client-runtime/client");
		//#region lib/types/client/settings-scope.js
		/**
		* Host transport for the settings-namespace scope contract. The contract types
		* live in `dsh-client-runtime` (the common dependency of every feature that
		* owns a preference); this file owns the wire behavior and the invalidation
		* subscription, both of which are Settings-surface concerns.
		*/
		/**
		* Serializes one namespace's Host reads and writes behind a snapshot store.
		* Reads never block plugin activation; writes carry the latest known
		* namespace revision and teardown waits for the operation already crossing
		* the wire.
		*/
		var SettingsScopeController = class {
			api;
			spec;
			persistence;
			store;
			tail = Promise.resolve();
			readGeneration = 0;
			writeGeneration = 0;
			disposed = false;
			/**
			* @param api - settings wire face.
			* @param spec - namespace identity and optional narrowing decoder.
			* @param persistence - remote browsers remain process-local because settings RPCs are loopback-only.
			*/
			constructor(api, spec, persistence = "host") {
				this.api = api;
				this.spec = spec;
				this.persistence = persistence;
				this.store = (0, _deepseek_ai_dsh_client_runtime_client.createSnapshotStore)({
					status: persistence === "host" ? "loading" : "unavailable",
					value: void 0,
					base: void 0,
					user: void 0,
					revision: void 0,
					writable: false,
					mode: persistence
				});
			}
			/** @returns the current sync snapshot (stable reference until the next change). */
			getSnapshot() {
				return this.store.getSnapshot();
			}
			/**
			* Observe snapshot replacements.
			* @param listener - invoked after each snapshot change.
			* @returns the disposer removing this listener.
			*/
			subscribe(listener) {
				return this.store.subscribe(listener);
			}
			/**
			* Queue a Host refresh; a newer read or user write suppresses stale publication.
			* @returns settlement after the queued read completes or is skipped.
			*/
			load() {
				const generation = ++this.readGeneration;
				return this.enqueue(() => this.read(generation));
			}
			/**
			* Queue one field write; see {@link SettingsScope.set} for the ordering,
			* revision, and recovery contract.
			* @param field - scalar field inside the namespace section.
			* @param value - JSON-shaped value selected by the user.
			* @returns settlement after the write and any latest-write recovery read.
			*/
			set(field, value) {
				return this.write({
					op: "set",
					path: [field],
					value
				});
			}
			/**
			* Queue one field clear; see {@link SettingsScope.unset} for the ordering,
			* revision, and recovery contract.
			* @param field - scalar field inside the namespace section.
			* @returns settlement after the clear and any latest-write recovery read.
			*/
			unset(field) {
				return this.write({
					op: "unset",
					path: [field]
				});
			}
			write(op) {
				this.readGeneration += 1;
				const generation = ++this.writeGeneration;
				return this.enqueue(async () => {
					const revision = this.getSnapshot().revision;
					let response;
					try {
						response = await this.api.settings.mutate({
							ns: this.spec.namespace,
							ops: [op],
							...revision === void 0 ? {} : { expectedRevision: revision }
						});
					} catch (_settingsWriteFailure) {
						if (!this.disposed && generation === this.writeGeneration) await this.read(++this.readGeneration);
						return;
					}
					if (!response.result.ok) {
						if (!this.disposed && generation === this.writeGeneration) await this.read(++this.readGeneration);
						return;
					}
					this.accept(response.result.value, generation === this.writeGeneration);
				});
			}
			/**
			* Stop queued operations and wait for the current wire call to settle.
			* @returns settlement after the controller reaches quiescence.
			*/
			async dispose() {
				this.disposed = true;
				this.readGeneration += 1;
				this.writeGeneration += 1;
				await this.tail;
			}
			enqueue(operation) {
				if (this.persistence === "memory" || this.disposed) return Promise.resolve();
				const task = this.tail.then(async () => {
					if (this.disposed) return;
					await operation();
				});
				this.tail = task.catch(() => {});
				return task;
			}
			async read(generation) {
				let response;
				try {
					response = await this.api.settings.describe({});
				} catch (error) {
					if (!this.disposed && generation === this.readGeneration) this.fail(error);
					return;
				}
				if (this.disposed) return;
				if (!response.result.ok) { if (generation === this.readGeneration) this.fail(new Error(response.result.error.message)); return; }
				const { namespaces, writable } = response.result.value;
				const view = namespaces.find((candidate) => candidate.ns === this.spec.namespace);
				const publish = generation === this.readGeneration;
				if (view === void 0) {
					if (publish) this.store.update((draft) => {
						draft.status = "unavailable";
						draft.writable = writable;
					});
					return;
				}
				this.accept(view, publish, writable);
			}
			accept(view, publish, writable) {
				let decoded;
				try { decoded = publish ? this.decode(view) : void 0; } catch (error) { this.fail(error); return; }
				if (publish && decoded === void 0) { this.fail(new Error("设置数据无法解析，请重新读取")); return; }
				this.store.update((draft) => {
					draft.revision = view.revision;
					draft.base = view.base;
					draft.user = view.user;
					if (writable !== void 0) draft.writable = writable;
					if (decoded === void 0) return;
					draft.status = "ready";
					draft.error = null;
					draft.value = decoded;
				});
			}
			fail(error) {
				this.store.update(draft => { draft.status = "error"; draft.error = error instanceof Error ? error.message : String(error); draft.writable = false; });
			}
			decode(view) {
				if (this.spec.decode !== void 0) return this.spec.decode(view.value);
				if (typeof view.value !== "object" || view.value === null || Array.isArray(view.value)) return void 0;
				let failure;
				try {
					failure = (0, _deepseek_ai_dsh_client_schema_form.validateDraft)((0, _deepseek_ai_dsh_client_schema_form.rehydrateSchema)(view.schema), view.value);
				} catch (_malformedSchemaEnvelope) {
					return;
				}
				return failure === void 0 ? view.value : void 0;
			}
		};
		/**
		* The settings domain's base service. Features that own a preference reach the
		* settings transport through this service rather than a shared function: the
		* client bundle purity gate forbids cross-plugin value imports and directs
		* cross-plugin collaboration through cordis services
		* (`packages/client/tsdown.client.ts`).
		*/
		var SettingsScopeBinder = class extends _deepseek_ai_cordis.Service {
            controls=Object.freeze({Switch:SettingsSwitch});
			/**
			* @param ctx - the providing plugin's context.
			*/
			constructor(ctx) {
				super(ctx, "settingsScope");
			}
			/**
			* Bind one namespace scope to settings and connection invalidations on the
			* CALLER's plugin lifecycle — the service proxy binds `this.ctx` to the
			* caller at call time, so the scope's disposer belongs to the calling fiber.
			* Listeners exist before the initial background read starts, so activation
			* never blocks on the settings transport. The caller injects `connection`
			* for the transport and `remote` for the forwarded settings invalidation.
			* @param spec - domain-owned namespace contract.
			* @returns the bound scope consumed by the domain's services and rows.
			*/
			bind(spec) {
				const ctx = this.ctx;
				const connection = ctx.get("connection");
				const controller = new SettingsScopeController(connection.api, spec, connection.isLoopback ? "host" : "memory");
				ctx.effect(() => {
					const refresh = (namespace) => {
						if (namespace !== void 0 && namespace !== spec.namespace) return;
						controller.load();
					};
					const disposers = [ctx.get("remote").$on("settings/document-updated", refresh), ctx.on("connection/reset", () => {
						refresh();
					})];
					controller.load();
					return async () => {
						for (const dispose of disposers) dispose();
						await controller.dispose();
					};
				}, `ui-settings: ${spec.namespace} settings scope`);
				return controller;
			}
		};
		//#endregion
		//#region lib/types/client/index.js
		/**
		* Required services: none. The transport is resolved per caller through
		* `this.ctx` at `bind` time, so this plugin waits for nothing.
		*/
		const inject = [];
		/**
		* Provide the settings-namespace scope service.
		*
		* Constructing the service in this plugin's fiber keeps its traced methods
		* bound to each consuming plugin's context.
		* @param ctx - client root context.
		*/
		function apply(ctx) {
            if(typeof document!=="undefined"&&!document.querySelector("style[data-dsh-settings-shared]")){const style=document.createElement("style");style.dataset.dshSettingsShared="";style.textContent="\n.dshSettingsDisclosure{box-sizing:border-box;border:1px solid var(--dsw-alias-border-l2);border-radius:12px;background:var(--dsw-alias-bg-base);padding:12px 14px;min-width:0;color:var(--dsw-alias-label-primary);font-size:13px;line-height:20px}.dshSettingsDisclosure>summary{cursor:pointer;font-size:14px;font-weight:500;list-style:revert}.dshSettingsDisclosure[open]>summary{margin-bottom:16px}\n.dswSuiteSettings,.dbs-settings-card{box-sizing:border-box;max-width:100%;min-width:0;font-size:13px;line-height:20px}.dswSuiteSettings h3,.dbs-settings-card h3{font-size:14px;font-weight:500;margin:0 0 8px}.dswSuiteSettings p,.dbs-setting-hint{color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:18px;margin:4px 0 12px}.dswSuiteSettings .dswSuiteSettings{border:0;padding:0;margin-bottom:20px}.dswSuiteSettings .dswSuiteSetting{grid-template-columns:minmax(0,1fr) minmax(0,220px);gap:12px}.dswSuiteButton{box-sizing:border-box;white-space:nowrap;flex-shrink:0;height:32px;border:1px solid var(--dsw-alias-border-l2);border-radius:16px;background:transparent;color:var(--dsw-alias-label-primary);padding:0 12px;cursor:pointer;font:inherit;font-size:13px;text-decoration:none;display:inline-flex;align-items:center;justify-content:center}.dswSuiteButton:hover{background:var(--dsw-alias-interactive-bg-hover)}.dswSuiteSettings input:not([type=checkbox]),.dswSuiteSettings select{box-sizing:border-box;min-width:0;width:100%;height:32px;font:inherit;padding:4px 10px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary)}.dswSuiteSettings .dswSuiteSetting>div:last-child{min-width:0;display:flex;align-items:center;gap:6px}.dswSuiteToolbar{display:flex;gap:6px;align-items:center;flex-wrap:wrap}.dswSuiteSettings button:disabled,.dswSuiteSettings input:disabled{opacity:.45;cursor:default}.dshCodexUsage{font-size:13px;line-height:20px}.dshCodexUsage>strong{font-weight:500}.dshCodexUsage progress{height:6px;accent-color:var(--dsw-alias-brand-primary)}\n@media(max-width:700px){.dswSuiteSettings .dswSuiteSetting{grid-template-columns:minmax(0,1fr)}.dswSuiteSettings .dswSuiteSetting>div:last-child{justify-content:flex-start}}\n\n.dshSettingsSwitch.dshSettingsSwitch{box-sizing:border-box;position:relative;flex:0 0 auto;display:block;width:36px;min-width:36px;max-width:36px;height:20px;min-height:20px;max-height:20px;padding:2px;border:0;border-radius:10px;corner-shape:round;background:var(--dsw-alias-border-l3);cursor:pointer}.dshSettingsSwitch[aria-checked=true]{background:var(--dsw-alias-brand-primary)}.dshSettingsSwitch:disabled{cursor:default;opacity:.5}.dshSettingsSwitch:focus-visible{outline:2px solid var(--dsw-alias-brand-primary);outline-offset:2px}.dshSettingsSwitchThumb{display:block;width:16px;height:16px;border-radius:50%;corner-shape:round;background:var(--dsw-alias-label-primary-foreground);transition:transform 120ms ease}.dshSettingsSwitch[aria-checked=true] .dshSettingsSwitchThumb{transform:translateX(16px)}\n.dswSuiteSettings .dswSuiteSetting{display:flex;justify-content:space-between;align-items:center;gap:20px;padding:14px 0}.dswSuiteSettings .dswSuiteSetting>span,.dswSuiteSettings .dswSuiteSetting>div:first-child{flex:1;min-width:0}.dswSuiteSettingControl{flex:none;max-width:240px}.dswSuiteSettings .dswSuiteSettingControl>input:not([type=checkbox]){width:160px}.dswSuiteSettings .dswSuiteSetting>div:last-child{flex:none}.dswSuiteSettings .dswSuiteSetting strong{font-size:14px;font-weight:500}.dswSuiteSettings .dswSuiteSetting p{font-size:12px;line-height:18px;margin:4px 0 0}.dswSuiteSettings h3{font-size:16px;line-height:24px;font-weight:500;margin:0 0 8px}\n.dshWorkspaceForm{display:flex;flex-direction:column;gap:16px;font-size:14px;line-height:22px;color:var(--dsw-alias-label-primary)}.dshWorkspaceForm p{margin:0}.dshWorkspacePath{padding:10px 12px;overflow-wrap:anywhere;background:var(--dsw-alias-bg-module-platform);border-radius:8px}.dshWorkspaceAdvanced{display:flex;align-items:center;gap:8px;border:0;background:transparent;color:var(--dsw-alias-label-secondary);padding:0;font:inherit;cursor:pointer}.dshWorkspaceFields{display:flex;flex-direction:column;gap:8px}.dshWorkspaceFields label{font-size:12px;color:var(--dsw-alias-label-secondary)}.dshWorkspaceLocation{display:flex;align-items:center;gap:8px}.dshWorkspaceLocation input{flex:1;min-width:0;width:100%;box-sizing:border-box;height:36px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-base);color:inherit;padding:0 10px;font:inherit}.dshWorkspaceForm .dshWorkspaceHint{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}\n\n.dshSettingsDisclosure{padding:16px}.dshSettingsDisclosure>summary{display:flex;align-items:center;justify-content:space-between;gap:12px;list-style:none;font-size:14px;line-height:20px}.dshSettingsDisclosure>summary::-webkit-details-marker{display:none}.dshSettingsDisclosure>summary::after{content:\"\";flex:none;width:6px;height:6px;margin-right:3px;border-right:1px solid var(--dsw-alias-label-tertiary);border-bottom:1px solid var(--dsw-alias-label-tertiary);transform:rotate(45deg)}.dshSettingsDisclosure[open]>summary::after{transform:rotate(225deg)}.dshSettingsDisclosure>.dswSuiteSettings,.dshSettingsDisclosure>.dbs-settings-card{border:0;border-radius:0;padding:0;background:transparent}\n\n.dswSuiteSettings .dswSuiteSetting>select{flex:0 1 220px;width:220px;max-width:60%}.dswSuiteSettings .dswSuiteSetting>span{flex:1 0 100px}.dswSuiteSettings .dswSuiteSettingControl{max-width:60%}@media(max-width:520px){.dswSuiteSettings .dswSuiteSetting{flex-wrap:wrap;gap:8px}.dswSuiteSettings .dswSuiteSetting>select,.dswSuiteSettings .dswSuiteSettingControl{max-width:100%}}\n";document.head.appendChild(style);}
			new SettingsScopeBinder(ctx);
		}
		//#endregion
		exports.SettingsScopeBinder = SettingsScopeBinder;
        exports.SettingsSwitch = SettingsSwitch;
		exports.SettingsScopeController = SettingsScopeController;
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map