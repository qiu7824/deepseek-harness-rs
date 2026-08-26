window.__ModuleLoader__.load({
	id: "@deepseek-ai/dsh-client-ui-settings-general",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		let _deepseek_ai_dsh_client_ui_slots = require("@deepseek-ai/dsh-client-ui-slots");
		let _deepseek_ai_dsh_client_web_react = require("@deepseek-ai/dsh-client-web-react");
		let react_jsx_runtime = require("react/jsx-runtime");
		let react = require("react");
		let _deepseek_ai_dsh_client_ui_primitives = require("@deepseek-ai/dsh-client-ui-primitives");
		let _deepseek_ai_dsh_client_runtime_client = require("@deepseek-ai/dsh-client-runtime/client");
		//#region ../../../node_modules/.pnpm/clsx@2.1.1/node_modules/clsx/dist/clsx.mjs
		function r(e) {
			var t, f, n = "";
			if ("string" == typeof e || "number" == typeof e) n += e;
			else if ("object" == typeof e) if (Array.isArray(e)) {
				var o = e.length;
				for (t = 0; t < o; t++) e[t] && (f = r(e[t])) && (n && (n += " "), n += f);
			} else for (f in e) e[f] && (n && (n += " "), n += f);
			return n;
		}
		function clsx() {
			for (var e, t, f = 0, n = "", o = arguments.length; f < o; f++) (e = arguments[f]) && (t = r(e)) && (n && (n += " "), n += t);
			return n;
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-settings-general\src\client\SettingsRoot.module.css.mjs
		const css$3 = "._7h7_Oq_trigger{box-sizing:border-box;cursor:pointer;width:calc(100% + 8px);height:34px;color:var(--dsw-alias-label-primary);background:0 0;border:none;border-radius:12px;flex:none;align-items:center;gap:8px;margin:4px -4px;padding:6px 2px 6px 10px;font-family:inherit;font-size:14px;line-height:22px;display:flex;overflow:hidden}._7h7_Oq_trigger:hover{background:var(--dsw-alias-interactive-bg-hover)}._7h7_Oq_trigger._7h7_Oq_rail{border-radius:50%;justify-content:center;gap:0;width:36px;height:36px;margin:8px 0 10px;padding:0}._7h7_Oq_triggerLabel{white-space:nowrap;overflow:hidden}._7h7_Oq_overlay{z-index:1000;justify-content:center;align-items:center;display:flex;position:fixed;inset:0}._7h7_Oq_mask{background:var(--dsw-alias-bg-mask-1);backdrop-filter:var(--dsw-mask-blur);position:absolute;inset:0}._7h7_Oq_panel{z-index:1;background:var(--dsw-alias-bg-layer-2);width:800px;max-width:calc(100vw - 48px);height:min(800px,100vh - 48px);box-shadow:var(--dsw-shadow-lv3);--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2);--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2);border-radius:24px;display:flex;position:relative;overflow:hidden}._7h7_Oq_nav{box-sizing:border-box;flex-direction:column;flex:none;gap:18px;width:188px;padding:22px 12px 0;display:flex}._7h7_Oq_navTitle{color:var(--dsw-alias-label-primary);padding:0 12px;font-size:16px;font-weight:500;line-height:24px}._7h7_Oq_navList{flex-direction:column;gap:4px;display:flex}._7h7_Oq_navCell{box-sizing:border-box;cursor:pointer;height:40px;color:var(--dsw-alias-label-primary);text-align:left;background:0 0;border:none;border-radius:12px;align-items:center;gap:8px;padding:9px 16px 9px 12px;font-family:inherit;font-size:14px;font-weight:400;line-height:22px;display:flex}._7h7_Oq_navCell:hover{background:var(--dsw-specific-sidebar-nav-item-hover)}._7h7_Oq_navCell._7h7_Oq_active{background:var(--dsw-specific-sidebar-nav-item-active)}._7h7_Oq_navIcon{flex:none}._7h7_Oq_navLabel{white-space:nowrap;text-overflow:ellipsis;flex:1;min-width:0;overflow:hidden}._7h7_Oq_content{flex-direction:column;flex:1;min-width:0;display:flex}._7h7_Oq_header{box-sizing:border-box;flex:none;justify-content:space-between;align-items:flex-start;gap:8px;height:54px;padding:20px 14px 8px 10px;display:flex}._7h7_Oq_actions{justify-content:flex-end;align-items:center;gap:8px;min-width:0;margin-left:auto;display:flex}._7h7_Oq_close{cursor:pointer;width:28px;height:28px;color:var(--dsw-alias-label-primary);background:0 0;border:none;border-radius:28px;justify-content:center;align-items:center;padding:0;display:inline-flex}._7h7_Oq_close:hover{background:var(--dsw-alias-interactive-bg-hover)}._7h7_Oq_options{flex:1;min-height:0;padding:0 24px 24px;overflow-y:auto}._7h7_Oq_hiddenLabel{clip:rect(0 0 0 0);white-space:nowrap;width:1px;height:1px;position:absolute;overflow:hidden}";
		const tagId$3 = "@deepseek-ai/dsh-client-ui-settings-general/SettingsRoot.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId$3) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-settings-general";
			tag.dataset.pluginCss = tagId$3;
			tag.textContent = css$3;
			document.head.appendChild(tag);
		}
		var SettingsRoot_module_css_default = {
			"navIcon": "_7h7_Oq_navIcon",
			"navTitle": "_7h7_Oq_navTitle",
			"actions": "_7h7_Oq_actions",
			"panel": "_7h7_Oq_panel",
			"header": "_7h7_Oq_header",
			"nav": "_7h7_Oq_nav",
			"trigger": "_7h7_Oq_trigger",
			"navList": "_7h7_Oq_navList",
			"active": "_7h7_Oq_active",
			"overlay": "_7h7_Oq_overlay",
			"rail": "_7h7_Oq_rail",
			"triggerLabel": "_7h7_Oq_triggerLabel",
			"navLabel": "_7h7_Oq_navLabel",
			"close": "_7h7_Oq_close",
			"options": "_7h7_Oq_options",
			"mask": "_7h7_Oq_mask",
			"content": "_7h7_Oq_content",
			"navCell": "_7h7_Oq_navCell",
			"hiddenLabel": "_7h7_Oq_hiddenLabel"
		};
		//#endregion
		//#region lib/types/client/SettingsRoot.js
		/**
		* Settings shell root: the sidebar-foot trigger row plus the centered modal
		* panel (figma 501:29947, 1080x700) with the section nav rail. The shell is
		* a pure composition face — every piece of text (trigger label, panel title,
		* close label, sections) arrives from registrants through slots; accessible
		* names resolve to that content (trigger: its own text; dialog:
		* aria-labelledby the title node; close: visually-hidden slot text). Modal
		* open state and the active section id are component-local viewing state;
		* the onboarding coordinator mounts exactly one ordered registrant while the
		* sessions-derived empty-Hero fact is active. Visible dialog chrome belongs
		* to the step, so a mounted-but-deciding step paints nothing here.
		*/
		/** Nav glyph by section id; unknown ids fall back to the settings gear. */
		function navIcon(id) {
			if (id === "models") return (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconDataOutline16, {
				className: SettingsRoot_module_css_default.navIcon,
				size: 16
			});
			if (id === "agent-presets") return (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconAgentPresetOutline16, {
				className: SettingsRoot_module_css_default.navIcon,
				size: 16
			});
			if (id === "plugins") return (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconPersonalizationOutline16, {
				className: SettingsRoot_module_css_default.navIcon,
				size: 16
			});
			return (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconSettingsOutline16, {
				className: SettingsRoot_module_css_default.navIcon,
				size: 16
			});
		}
		/**
		* The modal layer: full-viewport mask + centered panel. Close paths: the
		* header button, a mask click, and document-level Escape (mounted only while
		* open, so the listener lifetime is the panel's).
		*/
		function SettingsPanel({ rows, renderSlot, activeId, onSelect, onClose }) {
			const active = rows.find((r) => r.id === activeId)?.id ?? rows[0]?.id;
			const titleId = (0, react.useId)();
			(0, react.useEffect)(() => {
				const onKeyDown = (e) => {
					if (e.key === "Escape") onClose();
				};
				document.addEventListener("keydown", onKeyDown);
				return () => {
					document.removeEventListener("keydown", onKeyDown);
				};
			}, [onClose]);
			const closeButton = (0, react.useRef)(null);
			(0, react.useEffect)(() => {
				closeButton.current?.focus();
			}, []);
			return (0, react_jsx_runtime.jsxs)("div", {
				className: SettingsRoot_module_css_default.overlay,
				role: "presentation",
				children: [(0, react_jsx_runtime.jsx)("div", {
					className: SettingsRoot_module_css_default.mask,
					"aria-hidden": "true",
					onClick: onClose
				}), (0, react_jsx_runtime.jsxs)("div", {
					className: SettingsRoot_module_css_default.panel,
					role: "dialog",
					"aria-modal": "true",
					"aria-labelledby": titleId,
					children: [(0, react_jsx_runtime.jsxs)("nav", {
						className: SettingsRoot_module_css_default.nav,
						children: [(0, react_jsx_runtime.jsx)("div", {
							className: SettingsRoot_module_css_default.navTitle,
							id: titleId,
							children: renderSlot("settings.header", {})
						}), (0, react_jsx_runtime.jsx)("div", {
							className: SettingsRoot_module_css_default.navList,
							children: rows.map((row) => (0, react_jsx_runtime.jsxs)("button", {
								type: "button",
								className: clsx(SettingsRoot_module_css_default.navCell, row.id === active && SettingsRoot_module_css_default.active),
								"aria-current": row.id === active ? "true" : void 0,
								onClick: () => {
									onSelect(row.id);
								},
								children: [navIcon(row.id), (0, react_jsx_runtime.jsx)("span", {
									className: SettingsRoot_module_css_default.navLabel,
									children: row.label
								})]
							}, row.id))
						})]
					}), (0, react_jsx_runtime.jsxs)("div", {
						className: SettingsRoot_module_css_default.content,
						children: [(0, react_jsx_runtime.jsxs)("div", {
							className: SettingsRoot_module_css_default.header,
							children: [(0, react_jsx_runtime.jsx)("div", {
								className: SettingsRoot_module_css_default.actions,
								children: renderSlot("settings.action", {})
							}), (0, react_jsx_runtime.jsxs)("button", {
								ref: closeButton,
								type: "button",
								className: SettingsRoot_module_css_default.close,
								onClick: onClose,
								children: [(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconCloseOutline16, { size: 14 }), (0, react_jsx_runtime.jsx)("span", {
									className: SettingsRoot_module_css_default.hiddenLabel,
									children: renderSlot("settings.close", {})
								})]
							})]
						}), (0, react_jsx_runtime.jsx)("div", {
							className: SettingsRoot_module_css_default.options,
							children: active !== void 0 && renderSlot("settings.section", { close: onClose }, { only: active })
						})]
					})]
				})]
			});
		}
		/**
		* Render the settings trigger and panel.
		* @param props - composed slot props (contract/slots.ts).
		* @returns the settings shell element tree.
		*/
		function SettingsRoot(props) {
			const { wide, useSections, useOnboardingSteps, useSessions, renderSlot } = props;
			const [open, setOpen] = (0, react.useState)(false);
			const [activeId, setActiveId] = (0, react.useState)(void 0);
			const [completedOnboarding, setCompletedOnboarding] = (0, react.useState)(() => /* @__PURE__ */ new Set());
			const close = (0, react.useCallback)(() => {
				setOpen(false);
				setActiveId(void 0);
			}, []);
			const openSection = (0, react.useCallback)((id) => {
				setActiveId(id);
				setOpen(true);
			}, []);
			const rows = useSections((s) => s);
			const onboardingSteps = useOnboardingSteps((s) => s);
			const onboardingActive = useSessions((state) => state.phase === "ready" && (state.current === void 0 || state.byId[state.current]?.blank === true));
			const onboardingStep = onboardingActive ? onboardingSteps.find((step) => !completedOnboarding.has(step.id)) : void 0;
			(0, react.useEffect)(() => {
				if (onboardingActive) return;
				setCompletedOnboarding(/* @__PURE__ */ new Set());
			}, [onboardingActive]);
			const completeOnboardingStep = (0, react.useCallback)((id) => {
				setCompletedOnboarding((previous) => {
					if (previous.has(id)) return previous;
					return new Set([...previous, id]);
				});
			}, []);
			return (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
				(0, react_jsx_runtime.jsx)("button", {
					type: "button",
					className: clsx(SettingsRoot_module_css_default.trigger, !wide && SettingsRoot_module_css_default.rail),
					"aria-haspopup": "dialog",
					"aria-expanded": open,
					onClick: () => {
						setOpen(true);
					},
					children: renderSlot("settings.trigger", { wide })
				}),
				open && (0, react_jsx_runtime.jsx)(SettingsPanel, {
					rows,
					renderSlot,
					activeId,
					onSelect: setActiveId,
					onClose: close
				}),
				onboardingStep !== void 0 && renderSlot("settings.onboarding", {
					stepId: onboardingStep.id,
					complete: () => {
						completeOnboardingStep(onboardingStep.id);
					},
					openSection
				}, { only: onboardingStep.id })
			] });
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-settings-general\src\client\chrome.module.css.mjs
		const css$2 = ".mw0v-a_triggerLabel{white-space:nowrap;overflow:hidden}";
		const tagId$2 = "@deepseek-ai/dsh-client-ui-settings-general/chrome.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId$2) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-settings-general";
			tag.dataset.pluginCss = tagId$2;
			tag.textContent = css$2;
			document.head.appendChild(tag);
		}
		var chrome_module_css_default = { "triggerLabel": "mw0v-a_triggerLabel" };
		//#endregion
		//#region lib/types/client/chrome.js
		/**
		* Shell chrome content registered into the shell's trigger/header seats: the
		* trigger row icon + label (figma sidebar foot) and the panel title text.
		* The shell renders the surrounding chrome (button, nav heading row) and
		* reads each entry's `label` option for aria text.
		*/
		/**
		* Render the trigger row content (icon; label only in the wide column).
		* @param props - composed slot props.
		* @returns the trigger content fragment.
		*/
		function TriggerContent({ wide, t }) {
			return (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [wide ? (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconSettingsOutline16, { size: 16 }) : (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconSettingsOutline14, { size: 18 }), wide && (0, react_jsx_runtime.jsx)("span", {
				className: chrome_module_css_default.triggerLabel,
				children: t("trigger")
			})] });
		}
		/**
		* Render the panel title text.
		* @param props - composed slot props.
		* @returns the title text node.
		*/
		function HeaderContent({ t }) {
			return (0, react_jsx_runtime.jsx)(react_jsx_runtime.Fragment, { children: t("title") });
		}
		/**
		* Render the close button's visually-hidden label text.
		* @param props - composed slot props.
		* @returns the label text node.
		*/
		function CloseLabel({ t }) {
			return (0, react_jsx_runtime.jsx)(react_jsx_runtime.Fragment, { children: t("close") });
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-settings-general\src\client\GeneralSection.module.css.mjs
		const css$1 = ".aRTrVa_section{flex-direction:column;width:100%;display:flex}.aRTrVa_section>[data-slot=\"settings.general.item\"]>:last-child{border-bottom:none}";
		const tagId$1 = "@deepseek-ai/dsh-client-ui-settings-general/GeneralSection.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId$1) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-settings-general";
			tag.dataset.pluginCss = tagId$1;
			tag.textContent = css$1;
			document.head.appendChild(tag);
		}
		var GeneralSection_module_css_default = { "section": "aRTrVa_section" };
		//#endregion
		//#region lib/types/client/GeneralSection.js
		/**
		* Render the General section content column.
		* @param props - composed slot props (contract/slots.ts).
		* @returns the section element tree.
		*/
		function GeneralSection({ renderSlot }) {
			return (0, react_jsx_runtime.jsx)("div", {
				className: GeneralSection_module_css_default.section,
				children: renderSlot("settings.general.item", {})
			});
		}
		const securityCss = ".dshSecurity{box-sizing:border-box;flex-direction:column;gap:18px;width:100%;padding:4px 2px 24px;display:flex;color:var(--dsw-alias-label-primary)}.dshSecurity h2{font-size:18px;line-height:26px;margin:0}.dshSecuritySummary{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:10px}.dshSecurityCard{border:1px solid var(--dsw-alias-border-subtle);border-radius:12px;padding:12px;background:var(--dsw-alias-bg-layer-1)}.dshSecurityLabel{font-size:12px;color:var(--dsw-alias-label-tertiary)}.dshSecurityValue{font-size:14px;font-weight:500;margin-top:4px}.dshSecurityRule{font-size:13px;line-height:20px;color:var(--dsw-alias-label-secondary)}.dshSecurityTable{width:100%;border-collapse:collapse;font-size:12px}.dshSecurityTable th,.dshSecurityTable td{text-align:left;padding:8px;border-bottom:1px solid var(--dsw-alias-border-subtle);vertical-align:top}.dshSecurityTable th{color:var(--dsw-alias-label-tertiary);font-weight:400}.dshSecurityEmpty{font-size:13px;color:var(--dsw-alias-label-tertiary);padding:14px 0}.dshSecurityError{color:var(--dsw-alias-state-error-primary);font-size:13px}";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css='dsh-security-shield']") === null) {
			const tag = document.createElement("style");
			tag.dataset.pluginCss = "dsh-security-shield";
			tag.textContent = securityCss;
			document.head.appendChild(tag);
		}
		function SecuritySection({ api }) {
			const [state, setState] = (0, react.useState)({ loading: true, error: null, timeout: 30, preset: "workspace-write", rows: [] });
			(0, react.useEffect)(() => {
				let cancelled = false;
				(async () => {
					try {
						const [settingsReply, sessionsReply] = await Promise.all([api.settings.describe({}), api.sessions.list({})]);
						if (!settingsReply.result.ok) throw new Error(settingsReply.result.error.message);
						if (!sessionsReply.result.ok) throw new Error(sessionsReply.result.error.message);
						const namespaces = settingsReply.result.value.namespaces ?? [];
						const security = namespaces.find((entry) => entry.ns === "security")?.value ?? {};
						const permission = namespaces.find((entry) => entry.ns === "permission")?.value ?? {};
						const audits = [];
						for (const session of (sessionsReply.result.value.items ?? []).slice(0, 50)) {
							const historyReply = await api.sessions.history({ sessionId: session.sessionId });
							if (!historyReply.result.ok) continue;
							const pending = new Map();
							for (const event of historyReply.result.value.events ?? []) {
								if (event.type === "approval/asked") pending.set(event.data?.id, event);
								if (event.type === "approval/decided") {
									const asked = pending.get(event.data?.id);
									audits.push({ sessionId: session.sessionId, time: event.timestamp ?? asked?.timestamp, tool: asked?.data?.toolName ?? "-", reason: asked?.data?.reason ?? "-", outcome: event.data?.outcome ?? "unavailable" });
								}
							}
						}
						audits.sort((a, b) => String(b.time ?? "").localeCompare(String(a.time ?? "")));
						if (!cancelled) setState({ loading: false, error: null, timeout: security.approvalTimeoutSeconds ?? 30, preset: permission.defaultPreset ?? "workspace-write", rows: audits.slice(0, 100) });
					} catch (error) {
						if (!cancelled) setState((previous) => ({ ...previous, loading: false, error: error instanceof Error ? error.message : String(error) }));
					}
				})();
				return () => { cancelled = true; };
			}, [api]);
			return (0, react_jsx_runtime.jsxs)("section", { className: "dshSecurity", children: [
				(0, react_jsx_runtime.jsx)("h2", { children: "安全盾" }),
				(0, react_jsx_runtime.jsxs)("div", { className: "dshSecuritySummary", children: [
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityCard", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityLabel", children: "审批模式" }), (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityValue", children: state.preset })] }),
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityCard", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityLabel", children: "审批超时" }), (0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityValue", children: [state.timeout, " 秒"] })] }),
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityCard", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityLabel", children: "目录保护" }), (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityValue", children: "删除文件夹必须确认" })] })
				] }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityRule", children: "工具调用在执行边界应用沙箱与审批策略；审批超时、拒绝或无人应答时均失败关闭。每次询问和决定写入原会话审计事件。" }),
				state.error && (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityError", role: "alert", children: state.error }),
				state.loading ? (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityEmpty", children: "正在读取安全审计…" }) : state.rows.length === 0 ? (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityEmpty", children: "暂无审批记录" }) : (0, react_jsx_runtime.jsxs)("table", { className: "dshSecurityTable", children: [(0, react_jsx_runtime.jsx)("thead", { children: (0, react_jsx_runtime.jsxs)("tr", { children: [(0, react_jsx_runtime.jsx)("th", { children: "时间" }), (0, react_jsx_runtime.jsx)("th", { children: "工具" }), (0, react_jsx_runtime.jsx)("th", { children: "原因" }), (0, react_jsx_runtime.jsx)("th", { children: "结果" })] }) }), (0, react_jsx_runtime.jsx)("tbody", { children: state.rows.map((row, index) => (0, react_jsx_runtime.jsxs)("tr", { children: [(0, react_jsx_runtime.jsx)("td", { children: row.time ? new Date(row.time).toLocaleString() : "-" }), (0, react_jsx_runtime.jsx)("td", { children: row.tool }), (0, react_jsx_runtime.jsx)("td", { children: row.reason }), (0, react_jsx_runtime.jsx)("td", { children: row.outcome })] }, row.sessionId + index)) })] })
			] });
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-settings-general\src\client\SettingsDocumentAction.module.css.mjs
		const memoryCss = ".dshMemory{display:flex;flex-direction:column;gap:20px;width:100%;max-width:680px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshMemory h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshMemory h2+.dshMemoryHint,.dshMemory h2+.dshMemoryGrid{margin-top:-8px}.dshMemoryGrid{display:grid;grid-template-columns:120px minmax(0,1fr);gap:14px 16px;align-items:center}.dshMemoryGrid>label{font-size:13px;color:var(--dsw-alias-label-secondary);text-align:right;line-height:20px}.dshMemory input,.dshMemory select,.dshMemory textarea{box-sizing:border-box;width:100%;border:1px solid var(--dsw-alias-border-subtle);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px;transition:border-color .15s}.dshMemory input:focus,.dshMemory select:focus,.dshMemory textarea:focus{outline:none;border-color:var(--dsw-alias-border-emphasis)}.dshMemory input[type=checkbox]{width:auto;width:16px;height:16px;cursor:pointer;justify-self:start}.dshMemory input[type=number]{max-width:200px}.dshMemory input:disabled{opacity:.6;cursor:not-allowed}.dshMemory textarea{min-height:84px;resize:vertical}.dshMemoryToolbar{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshMemoryToolbar select{width:auto;min-width:130px}.dshMemoryToolbar label{display:inline-flex;align-items:center;gap:5px;font-size:13px;color:var(--dsw-alias-label-secondary);cursor:pointer}.dshMemoryToolbar label input[type=checkbox]{width:auto;width:15px;height:15px}.dshMemory button{border:1px solid var(--dsw-alias-border-subtle);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:6px 14px;font-size:13px;line-height:20px;cursor:pointer;transition:background .15s,border-color .15s}.dshMemory button:hover{background:var(--dsw-alias-interactive-bg-hover);border-color:var(--dsw-alias-border-emphasis)}.dshMemory button:active{transform:translateY(1px)}.dshMemoryList{display:flex;flex-direction:column;gap:10px}.dshMemoryItem{border:1px solid var(--dsw-alias-border-subtle);border-radius:12px;padding:14px 16px;display:flex;flex-direction:column;gap:10px;background:var(--dsw-alias-bg-layer-1)}.dshMemoryItemHead{display:flex;gap:10px;align-items:center}.dshMemoryItemHead strong{flex:1;font-size:14px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dshMemoryItemHead .dshMemoryBadge{font-size:11px;padding:2px 8px;border-radius:6px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary);white-space:nowrap}.dshMemoryItemHead button{padding:4px 10px;font-size:12px}.dshMemoryItem p{margin:0;font-size:13px;line-height:20px;white-space:pre-wrap;color:var(--dsw-alias-label-secondary)}.dshMemory .dshMemoryItem>input,.dshMemory .dshMemoryItem>textarea{margin:0}.dshMemoryError{color:var(--dsw-alias-state-error-primary);font-size:13px;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08))}.dshMemoryHint{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshMemoryDivider{height:1px;background:var(--dsw-alias-border-subtle);margin:4px 0}";
		if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='dsh-memory-settings']")) { const tag = document.createElement("style"); tag.dataset.pluginCss = "dsh-memory-settings"; tag.textContent = memoryCss; document.head.appendChild(tag); }
		const memoryFields = [["enabled","持久记忆","checkbox"],["userProfileEnabled","用户画像","checkbox"],["memoryBudget","记忆预算","number"],["profileBudget","画像预算","number"],["provider","记忆提供方","fixed","仅内置"],["contextEngine","上下文引擎","fixed","Compressor"],["autoCompact","自动压缩","checkbox"],["compactThreshold","压缩阈值","number"],["compactTarget","压缩目标","number"],["protectRecentMessages","保护最近消息","number"]];
		function MemorySection({ api }) {
			const [settings,setSettings]=(0,react.useState)(null),[entries,setEntries]=(0,react.useState)([]),[categories,setCategories]=(0,react.useState)([]),[scopes,setScopes]=(0,react.useState)(["default"]),[scope,setScope]=(0,react.useState)("default"),[category,setCategory]=(0,react.useState)(""),[draft,setDraft]=(0,react.useState)(null),[error,setError]=(0,react.useState)(null);
			const load=(0,react.useCallback)(async()=>{try{const [description,categoryReply,entryReply,roster]=await Promise.all([api.settings.describe({}),api.memory.categories({}),api.memory.list({scope,...category?{category}:{}}),api.agentPresets.list({})]);if(!description.result.ok)throw new Error(description.result.error.message);if(!categoryReply.result.ok)throw new Error(categoryReply.result.error.message);if(!entryReply.result.ok)throw new Error(entryReply.result.error.message);setSettings(description.result.value.namespaces.find(item=>item.ns==="memory")??null);setCategories(categoryReply.result.value.categories??[]);setEntries(entryReply.result.value.entries??[]);if(roster.result.ok)setScopes(["default",...(roster.result.value.presets??[]).map(item=>item.id).filter(id=>id!=="default")]);setError(null)}catch(cause){setError(cause instanceof Error?cause.message:String(cause))}},[api,scope,category]);
			(0,react.useEffect)(()=>{load()},[load]);
			const setOption=async(field,value)=>{if(!settings)return;const reply=await api.settings.mutate({ns:"memory",ops:[{op:"set",path:[field],value}],expectedRevision:settings.revision});if(!reply.result.ok){setError(reply.result.error.message);await load();return}setSettings(reply.result.value)};
			const save=async()=>{if(!draft)return;const entry={id:draft.id??"",scope:draft.scope,category:draft.category,title:draft.title,content:draft.content,enabled:draft.enabled,revision:draft.revision??0};const reply=await api.memory.upsert({entry,...draft.revision?{expectedRevision:draft.revision}:{}});if(!reply.result.ok){setError(reply.result.error.message);return}setDraft(null);await load()};
			const remove=async entry=>{if(!window.confirm(`删除记忆“${entry.title}”？`))return;const reply=await api.memory.remove({id:entry.id,expectedRevision:entry.revision});if(!reply.result.ok)setError(reply.result.error.message);await load()};
			return (0,react_jsx_runtime.jsxs)("section",{className:"dshMemory",children:[(0,react_jsx_runtime.jsx)("h2",{children:"记忆与上下文"}),settings&&(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryGrid",children:memoryFields.flatMap(([field,label,type,fixed])=>[(0,react_jsx_runtime.jsx)("label",{children:label},field+"-l"),type==="checkbox"?(0,react_jsx_runtime.jsx)("input",{type:"checkbox",checked:Boolean(settings.value[field]),onChange:event=>setOption(field,event.target.checked)},field):type==="fixed"?(0,react_jsx_runtime.jsx)("input",{value:fixed,disabled:true},field):(0,react_jsx_runtime.jsx)("input",{type:"number",value:settings.value[field],step:field.includes("Threshold")||field.includes("Target")?"0.05":"1",onChange:event=>setOption(field,Number(event.target.value))},field)])}),(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryDivider"}),(0,react_jsx_runtime.jsx)("h2",{children:"Agent 记忆管理"}),(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryHint",children:"可分类维护偏好、已有工具、已知错误、项目知识和操作约束。"}),(0,react_jsx_runtime.jsxs)("div",{className:"dshMemoryToolbar",children:[(0,react_jsx_runtime.jsx)("select",{value:scope,onChange:e=>setScope(e.target.value),children:scopes.map(id=>(0,react_jsx_runtime.jsx)("option",{value:id,children:id},id))}),(0,react_jsx_runtime.jsxs)("select",{value:category,onChange:e=>setCategory(e.target.value),children:[(0,react_jsx_runtime.jsx)("option",{value:"",children:"全部分类"}),...categories.map(item=>(0,react_jsx_runtime.jsx)("option",{value:item.id,children:item.label},item.id))]}),(0,react_jsx_runtime.jsx)("button",{onClick:()=>setDraft({scope,category:category||"custom",title:"",content:"",enabled:true}),children:"新增记忆"})]}),error&&(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryError",role:"alert",children:error}),draft&&(0,react_jsx_runtime.jsxs)("div",{className:"dshMemoryItem",children:[(0,react_jsx_runtime.jsx)("input",{value:draft.title,placeholder:"标题",onChange:e=>setDraft({...draft,title:e.target.value})}),(0,react_jsx_runtime.jsx)("textarea",{value:draft.content,placeholder:"记忆内容",onChange:e=>setDraft({...draft,content:e.target.value})}),(0,react_jsx_runtime.jsxs)("div",{className:"dshMemoryToolbar",children:[(0,react_jsx_runtime.jsx)("select",{value:draft.category,onChange:e=>setDraft({...draft,category:e.target.value}),children:categories.map(item=>(0,react_jsx_runtime.jsx)("option",{value:item.id,children:item.label},item.id))}),(0,react_jsx_runtime.jsx)("label",{children:[(0,react_jsx_runtime.jsx)("input",{type:"checkbox",checked:draft.enabled,onChange:e=>setDraft({...draft,enabled:e.target.checked})})," 启用"]}),(0,react_jsx_runtime.jsx)("button",{onClick:save,children:"保存"}),(0,react_jsx_runtime.jsx)("button",{onClick:()=>setDraft(null),children:"取消"})]})]}),(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryList",children:entries.length===0?(0,react_jsx_runtime.jsx)("div",{className:"dshMemoryHint",children:"暂无记忆"}):entries.map(entry=>(0,react_jsx_runtime.jsxs)("div",{className:"dshMemoryItem",children:[(0,react_jsx_runtime.jsxs)("div",{className:"dshMemoryItemHead",children:[(0,react_jsx_runtime.jsx)("strong",{children:entry.title}),(0,react_jsx_runtime.jsx)("span",{className:"dshMemoryBadge",children:categories.find(item=>item.id===entry.category)?.label??entry.category}),(0,react_jsx_runtime.jsx)("button",{onClick:()=>setDraft(entry),children:"编辑"}),(0,react_jsx_runtime.jsx)("button",{onClick:()=>remove(entry),children:"删除"})]}),(0,react_jsx_runtime.jsx)("p",{children:entry.content})]},entry.id))})]})
		}
		const css = ".S1Gy-G_action{align-items:center;gap:8px;min-width:0;display:flex}.S1Gy-G_error{max-width:180px;color:var(--dsw-alias-state-error-primary);text-overflow:ellipsis;white-space:nowrap;font-size:12px;line-height:18px;overflow:hidden}";
		const tagId = "@deepseek-ai/dsh-client-ui-settings-general/SettingsDocumentAction.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-settings-general";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var SettingsDocumentAction_module_css_default = {
			"error": "S1Gy-G_error",
			"action": "S1Gy-G_action"
		};
		//#endregion
		//#region lib/types/client/SettingsDocumentAction.js
		/** Optional settings-header action for opening a file-backed Host document. */
		/**
		* Render the open-document action only after Host metadata confirms document availability.
		* @param props - header owner props, localized copy, and injected document state.
		* @returns the action, or null while unavailable or unresolved.
		*/
		function SettingsDocumentAction({ controller, useSnapshot, t }) {
			const state = useSnapshot((snapshot) => snapshot);
			(0, react.useEffect)(() => {
				controller.load();
			}, [controller]);
			if (state.status !== "ready") return null;
			return (0, react_jsx_runtime.jsxs)("div", {
				className: SettingsDocumentAction_module_css_default.action,
				children: [state.error === null ? null : (0, react_jsx_runtime.jsx)("span", {
					className: SettingsDocumentAction_module_css_default.error,
					role: "alert",
					children: t("openDocument.error")
				}), (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button, {
					variant: "outline",
					size: "sm",
					disabled: state.opening,
					onClick: () => {
						controller.open();
					},
					children: t("openDocument")
				})]
			});
		}
		//#endregion
		//#region lib/types/client/settings-document-store.js
		/** State owner for the optional local settings-document action. */
		function messageOf(error) {
			return error instanceof Error ? error.message : String(error);
		}
		/** Loads local-document availability and invokes the pathless Host-owned open operation. */
		var SettingsDocumentStore = class {
			api;
			/** uSES-safe state source shared by the registered header action. */
			store = (0, _deepseek_ai_dsh_client_runtime_client.createSnapshotStore)({
				status: "idle",
				opening: false,
				error: null
			});
			generation = 0;
			/**
			* @param api - loopback settings wire face that reports and opens the provider document.
			*/
			constructor(api) {
				this.api = api;
			}
			/**
			* Load whether the current provider owns a local document.
			* @returns after the latest metadata response updates the store.
			*/
			async load() {
				const generation = ++this.generation;
				this.store.update((state) => {
					state.status = "loading";
					state.error = null;
				});
				try {
					const { result } = await this.api.settings.describe({});
					if (generation !== this.generation) return;
					if (!result.ok) {
						this.store.update((state) => {
							state.status = "unavailable";
							state.error = result.error.message;
						});
						return;
					}
					this.store.update((state) => {
						state.status = result.value.hasDocument ? "ready" : "unavailable";
						state.error = null;
					});
				} catch (error) {
					if (generation !== this.generation) return;
					this.store.update((state) => {
						state.status = "unavailable";
						state.error = messageOf(error);
					});
				}
			}
			/**
			* Open the loaded document once; concurrent gestures collapse behind the in-flight action.
			* @returns after the native-open request settles, or immediately when unavailable/already opening.
			*/
			async open() {
				const current = this.store.getSnapshot();
				if (current.status !== "ready" || current.opening) return;
				this.store.update((state) => {
					state.opening = true;
					state.error = null;
				});
				try {
					const response = await this.api.settings.openDocument({});
					if (!response.result.ok) throw new Error(response.result.error.message);
				} catch (error) {
					this.store.update((state) => {
						state.error = messageOf(error);
					});
				} finally {
					this.store.update((state) => {
						state.opening = false;
					});
				}
			}
		};
		/**
		* Refresh document availability after reconnect only when a surface has already requested it.
		* @param controller - optional loopback document state owner.
		*/
		function refreshDocumentIfLoaded(controller) {
			if (controller === void 0 || controller.store.getSnapshot().status === "idle") return;
			controller.load();
		}
		//#endregion
		//#region lib/types/client/locales.js
		/** Shell chrome and General-nav dictionaries; feature rows own their copy. */
		/** Simplified Chinese dictionary (the key-set source of truth). */
		const zh = {
			"trigger": "设置",
			"title": "设置",
			"close": "关闭",
			"openDocument": "打开配置文件",
			"openDocument.error": "无法打开配置文件",
			"general.nav": "通用设置"
		};
		/** English dictionary, checked complete against the zh key set. */
		const en = {
			"trigger": "Settings",
			"title": "Settings",
			"close": "Close",
			"openDocument": "Open configuration file",
			"openDocument.error": "Could not open configuration file",
			"general.nav": "General"
		};
		//#endregion
		//#region lib/types/client/index.js
		/** Dictionary namespace owned by this plugin (shell chrome + General copy). */
		const NS = "settings";
		/**
		* Required services (cordis fiber inject). The target slots are declared by
		* ui-settings' apply, whose activation order relative to this one is NOT
		* constrained; registrations depend on their slots through `slots.inject()`.
		*/
		const inject = [
			"slots",
			"locale",
			"connection"
		];
		/**
		* Register the `settings` dictionaries, the chrome content, and the General
		* section, each once its slot declaration is on the ledger.
		* @param ctx - client root context.
		*/
		function apply(ctx) {
			ctx.effect(() => ctx.locale.register(NS, {
				zh,
				en
			}), "ui-settings-general: dictionaries");
			const t = ctx.locale.bind(NS);
			const connection = ctx.get("connection");
			const documentController = connection.isLoopback ? new SettingsDocumentStore(connection.api) : void 0;
			const documentInjected = documentController === void 0 ? void 0 : (() => {
				const useSnapshot = (0, _deepseek_ai_dsh_client_web_react.bindSnapshotSelector)(documentController.store);
				return () => ({
					controller: documentController,
					useSnapshot
				});
			})();
			ctx.effect(() => ctx.on("connection/reset", () => {
				refreshDocumentIfLoaded(documentController);
			}), "ui-settings-general: metadata invalidations");
			let rowsVersion = -1;
			let rowsRevision = -1;
			let rows = [];
			let onboardingVersion = -1;
			let onboardingSteps = [];
			const shellInjected = () => ({ hooks: {
				sections: {
					getSnapshot: () => {
						const version = ctx.slots.getVersion("settings.section");
						const revision = ctx.locale.getSnapshot().revision;
						if (version !== rowsVersion || revision !== rowsRevision) {
							rowsVersion = version;
							rowsRevision = revision;
							rows = ctx.slots.entries("settings.section").map((e) => ({
								/* v8 ignore next -- list-slot registration requires id (SlotCore rejects an entry without one) */
								id: e.options.id ?? "",
								order: e.options.order ?? 0,
								label: (0, _deepseek_ai_dsh_client_ui_slots.resolveSlotLabel)(e.options.label) ?? ""
							})).sort((a, b) => a.order - b.order);
						}
						return rows;
					},
					subscribe: (listener) => {
						const offLedger = ctx.slots.subscribe("settings.section", listener);
						const offLocale = ctx.locale.subscribe(listener);
						return () => {
							offLedger();
							offLocale();
						};
					}
				},
				onboardingSteps: {
					getSnapshot: () => {
						const version = ctx.slots.getVersion("settings.onboarding");
						if (version !== onboardingVersion) {
							onboardingVersion = version;
							onboardingSteps = ctx.slots.entries("settings.onboarding").map((e) => ({
								/* v8 ignore next -- list-slot registration requires id */
								id: e.options.id ?? "",
								order: e.options.order ?? 0
							})).sort((a, b) => a.order - b.order);
						}
						return onboardingSteps;
					},
					subscribe: (listener) => ctx.slots.subscribe("settings.onboarding", listener)
				}
			} });
			ctx.slots.inject("sidebar.settings", () => ctx.slots.register({
				name: "sidebar.settings",
				children: {
					"settings.trigger": {
						kind: "single",
						scope: "root"
					},
					"settings.header": {
						kind: "single",
						scope: "root"
					},
					"settings.action": {
						kind: "list",
						scope: "root"
					},
					"settings.close": {
						kind: "single",
						scope: "root"
					},
					"settings.section": {
						kind: "list",
						scope: "root"
					},
					"settings.onboarding": {
						kind: "list",
						scope: "root"
					}
				},
				inject: shellInjected
			}, SettingsRoot));
			ctx.slots.inject("settings.trigger", () => ctx.slots.register({
				name: "settings.trigger",
				locale: NS
			}, TriggerContent));
			ctx.slots.inject("settings.header", () => ctx.slots.register({
				name: "settings.header",
				locale: NS
			}, HeaderContent));
			if (documentInjected !== void 0) ctx.slots.inject("settings.action", () => ctx.slots.register({
				name: "settings.action",
				id: "open-document",
				order: 0,
				locale: NS,
				inject: documentInjected
			}, SettingsDocumentAction));
			ctx.slots.inject("settings.close", () => ctx.slots.register({
				name: "settings.close",
				locale: NS
			}, CloseLabel));
			ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "general",
				order: 0,
				label: () => t("general.nav"),
				locale: NS,
				children: { "settings.general.item": {
					kind: "list",
					scope: "root"
				} }
			}, GeneralSection));
			ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "memory",
				order: 20,
				label: "记忆与上下文"
			}, () => (0, react_jsx_runtime.jsx)(MemorySection, { api: connection.api })));
			ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "security",
				order: 25,
				label: "安全盾"
			}, () => (0, react_jsx_runtime.jsx)(SecuritySection, { api: connection.api })));
		}
		//#endregion
		exports.SettingsDocumentStore = SettingsDocumentStore;
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map