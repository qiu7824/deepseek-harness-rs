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
		const connectionIndicatorCss = ".dshConnectionIndicator{box-sizing:border-box;display:inline-grid;grid-template-columns:14px max-content;align-items:center;column-gap:4px;height:32px;padding:0 10px;border:0;border-radius:8px;font:500 12px/18px inherit;white-space:nowrap}.dshConnectionIndicatorWarning{background:var(--dsw-alias-state-warn-tertiary);color:var(--dsw-alias-state-warn-label);cursor:pointer}.dshConnectionIndicatorWarning:focus-visible{outline:2px solid var(--dsw-alias-state-warn-label);outline-offset:2px}.dshConnectionIndicatorSuccess{background:var(--dsw-alias-state-success-tertiary);color:var(--dsw-alias-state-success-primary)}.dshConnectionIndicatorLabel{display:grid}.dshConnectionStateLabel,.dshConnectionHoverLabel{grid-area:1/1}.dshConnectionHoverLabel{visibility:hidden}.dshConnectionIndicatorWarning:is(:hover,:focus-visible) .dshConnectionStateLabel{visibility:hidden}.dshConnectionIndicatorWarning:is(:hover,:focus-visible) .dshConnectionHoverLabel{visibility:visible}";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css='dsh-connection-indicator']") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-settings-general";
			tag.dataset.pluginCss = "dsh-connection-indicator";
			tag.textContent = connectionIndicatorCss;
			document.head.appendChild(tag);
		}
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
		function ConnectionRecoveryIndicator({ state, reconnect, t }) {
			if (state !== "disconnected" && state !== "connecting" && state !== "recovered") return null;
			if (state === "recovered") return (0, react_jsx_runtime.jsxs)("div", {
				className: "dshConnectionIndicator dshConnectionIndicatorSuccess",
				role: "status",
				"aria-label": t("connection.connected"),
				children: [(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconCheckOutline16, { size: 14 }), (0, react_jsx_runtime.jsx)("span", { children: t("connection.connected") })]
			});
			const connecting = state === "connecting";
			return (0, react_jsx_runtime.jsxs)("button", {
				type: "button",
				className: "dshConnectionIndicator dshConnectionIndicatorWarning",
				"data-phase": state,
				"aria-label": t(connecting ? "connection.restart" : "connection.reconnect"),
				onClick: reconnect,
				children: [(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconWarningOutline16, { size: 14 }), (0, react_jsx_runtime.jsxs)("span", {
					className: "dshConnectionIndicatorLabel",
					children: [(0, react_jsx_runtime.jsx)("span", { className: "dshConnectionStateLabel", children: connecting ? `${t("connection.connecting")}…` : t("connection.error") }), (0, react_jsx_runtime.jsx)("span", { className: "dshConnectionHoverLabel", children: t("connection.retry") })]
				})]
			});
		}
		function SettingsRoot(props) {
			const { wide, reconnect, useConnectionState, useSections, useOnboardingSteps, useSessions, renderSlot, t } = props;
			const [open, setOpen] = (0, react.useState)(false);
			const [activeId, setActiveId] = (0, react.useState)(void 0);
			const [completedOnboarding, setCompletedOnboarding] = (0, react.useState)(() => /* @__PURE__ */ new Set());
			const [showRecovery, setShowRecovery] = (0, react.useState)(false);
			const triggerButton = (0, react.useRef)(null);
			const wasOpen = (0, react.useRef)(open);
			const close = (0, react.useCallback)(() => {
				setOpen(false);
				setActiveId(void 0);
			}, []);
			(0, react.useEffect)(() => {
				if (wasOpen.current && !open) triggerButton.current?.focus();
				wasOpen.current = open;
			}, [open]);
			const openSection = (0, react.useCallback)((id) => {
				setActiveId(id);
				setOpen(true);
			}, []);
			const rows = useSections((s) => s);
			const connectionState = useConnectionState((state) => state);
			const previousConnectionState = (0, react.useRef)(connectionState);
			const onboardingSteps = useOnboardingSteps((s) => s);
			const onboardingActive = useSessions((state) => state.phase === "ready" && (state.current === void 0 || state.byId[state.current]?.blank === true));
			const onboardingStep = onboardingActive ? onboardingSteps.find((step) => !completedOnboarding.has(step.id)) : void 0;
			(0, react.useEffect)(() => {
				if (onboardingActive) return;
				setCompletedOnboarding(/* @__PURE__ */ new Set());
			}, [onboardingActive]);
			(0, react.useLayoutEffect)(() => {
				const previous = previousConnectionState.current;
				previousConnectionState.current = connectionState;
				if (connectionState !== "connected") {
					setShowRecovery(false);
					return;
				}
				if (previous !== "disconnected" && previous !== "connecting") return;
				setShowRecovery(true);
				const timeout = window.setTimeout(() => {
					setShowRecovery(false);
				}, 2e3);
				return () => {
					window.clearTimeout(timeout);
				};
			}, [connectionState]);
			const completeOnboardingStep = (0, react.useCallback)((id) => {
				setCompletedOnboarding((previous) => {
					if (previous.has(id)) return previous;
					return new Set([...previous, id]);
				});
			}, []);
			let connectionIndicator;
			if (connectionState === "disconnected") connectionIndicator = "disconnected";
			else if (connectionState === "connecting") connectionIndicator = "connecting";
			else if (showRecovery) connectionIndicator = "recovered";
			return (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
				(0, react_jsx_runtime.jsxs)("div", {
					className: clsx(SettingsRoot_module_css_default.triggerRow, !wide && SettingsRoot_module_css_default.railRow),
					children: [(0, react_jsx_runtime.jsx)("button", {
						ref: triggerButton,
						type: "button",
						className: clsx(SettingsRoot_module_css_default.trigger, !wide && SettingsRoot_module_css_default.rail),
						"aria-haspopup": "dialog",
						"aria-expanded": open,
						onClick: () => {
							setOpen(true);
						},
						children: renderSlot("settings.trigger", { wide })
					}), (0, react_jsx_runtime.jsx)(ConnectionRecoveryIndicator, {
						state: wide ? connectionIndicator : void 0,
						reconnect,
						t
					})]
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
		const securityCss = ".dshSecurity{box-sizing:border-box;display:flex;flex-direction:column;gap:16px;width:100%;max-width:640px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshSecurityHeader{display:flex;flex-direction:column;gap:4px}.dshSecurity h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshSecurityIntro{font-size:12px;line-height:19px;color:var(--dsw-alias-label-tertiary)}.dshSecurityGroup{display:flex;flex-direction:column;border:1px solid var(--dsw-alias-border-subtle);border-radius:14px;background:var(--dsw-alias-bg-layer-1);overflow:hidden}.dshSecurityGroupTitle{padding:12px 16px 8px;font-size:12px;font-weight:600;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshSecurityRow{display:grid;grid-template-columns:minmax(150px,1fr) minmax(180px,240px);align-items:center;gap:16px;padding:12px 16px;border-top:1px solid var(--dsw-alias-border-subtle)}.dshSecurityRow:first-of-type{border-top:none}.dshSecurityRowText{min-width:0}.dshSecurityRowLabel{font-size:13px;font-weight:500;line-height:20px}.dshSecurityRowHint{margin-top:2px;font-size:11px;line-height:17px;color:var(--dsw-alias-label-tertiary)}.dshSecurityRow select,.dshSecurityRow input{box-sizing:border-box;width:100%;border:1px solid var(--dsw-alias-border-subtle);border-radius:8px;background:var(--dsw-alias-bg-base);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px}.dshSecurityRow input[type=number]{max-width:120px;justify-self:end}.dshSecurityNotice{padding:12px 14px;border-radius:12px;background:var(--dsw-alias-bg-layer-1);font-size:12px;line-height:19px;color:var(--dsw-alias-label-secondary)}.dshSecurityError{padding:9px 12px;border-radius:9px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08));color:var(--dsw-alias-state-error-primary);font-size:13px}.dshSecurityEmpty{font-size:13px;color:var(--dsw-alias-label-tertiary);padding:8px 0}@media(max-width:640px){.dshSecurityRow{grid-template-columns:1fr;gap:8px}.dshSecurityRow input[type=number]{justify-self:start}}";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css='dsh-security-shield']") === null) {
			const tag = document.createElement("style");
			tag.dataset.pluginCss = "dsh-security-shield";
			tag.textContent = securityCss;
			document.head.appendChild(tag);
		}
		function SecuritySection({ api }) {
			const [state, setState] = (0, react.useState)({ loading: true, error: null, namespace: null, timeout: 30, preset: "workspace-write" });
			const load = (0, react.useCallback)(async () => {
				try {
					const settingsReply = await api.settings.describe({});
					if (!settingsReply.result.ok) throw new Error(settingsReply.result.error.message);
					const namespaces = settingsReply.result.value.namespaces ?? [];
					const securityNamespace = namespaces.find((entry) => entry.ns === "security") ?? null;
					const security = securityNamespace?.value ?? {};
					const permission = namespaces.find((entry) => entry.ns === "permission")?.value ?? {};
					setState({ loading: false, error: null, namespace: securityNamespace, timeout: security.approvalTimeoutSeconds ?? 30, preset: permission.defaultPreset ?? "workspace-write" });
				} catch (error) {
					setState((previous) => ({ ...previous, loading: false, error: error instanceof Error ? error.message : String(error) }));
				}
			}, [api]);
			(0, react.useEffect)(() => { load(); }, [load]);
			const setField = async (field, value) => {
				if (!state.namespace) return;
				const reply = await api.settings.mutate({ ns: "security", ops: [{ op: "set", path: [field], value }], expectedRevision: state.namespace.revision });
				if (!reply.result.ok) {
					setState((previous) => ({ ...previous, error: reply.result.error.message }));
					await load();
					return;
				}
				setState((previous) => ({ ...previous, namespace: reply.result.value, timeout: reply.result.value.value?.approvalTimeoutSeconds ?? previous.timeout, error: null }));
			};
			const securityValue = (field, fallback) => state.namespace?.value?.[field] ?? fallback;
			const row = (id, label, hint, control) => (0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityRow", children: [(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityRowText", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityRowLabel", children: label }), (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityRowHint", children: hint })] }), control] }, id);
			const select = (id, field, fallback, options) => (0, react_jsx_runtime.jsx)("select", { id, value: securityValue(field, fallback), onChange: (event) => setField(field, event.target.value), children: options.map(([value, label]) => (0, react_jsx_runtime.jsx)("option", { value, children: label }, value)) });
			return (0, react_jsx_runtime.jsxs)("section", { className: "dshSecurity", children: [
				(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityHeader", children: [(0, react_jsx_runtime.jsx)("h2", { children: "安全盾" }), (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityIntro", children: "控制工具在执行前如何审批。修改会立即保存并应用到当前 Host。" })] }),
				state.namespace && (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityGroup", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityGroupTitle", children: "审批行为" }), row("timeout", "审批超时", "等待对话中确认的最长时间。超时后按无人确认策略处理。", (0, react_jsx_runtime.jsx)("input", { id: "security-timeout", type: "number", min: 5, max: 300, step: 1, value: securityValue("approvalTimeoutSeconds", 30), onChange: (event) => setField("approvalTimeoutSeconds", Number(event.target.value)) })), row("unattended", "无人确认", "没有可用浏览器、断线或超时后的默认处理。", select("security-unattended", "unattendedPolicy", "deny", [["deny", "拒绝（推荐）"], ["allow-safe-only", "仅允许安全操作"], ["allow-all", "允许可审批操作"]]))] }),
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityGroup", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityGroupTitle", children: "工具与路径" }), row("risk", "破坏性命令", "删除、重置、终止进程等高风险命令。", select("security-risk", "riskToolPolicy", "ask", [["ask", "对话中询问"], ["deny", "直接拒绝"]])), row("outside", "工作区外写入", "控制对当前工作区之外文件的修改。", select("security-outside-write", "outsideWritePolicy", "ask-directory", [["ask-directory", "按目录询问，可记忆"], ["ask-every-time", "每次询问"], ["deny", "直接拒绝"]])), row("sensitive", "敏感路径读取", ".env、SSH、云凭据等敏感位置；授权不会被记忆。", select("security-sensitive-read", "sensitiveReadPolicy", "ask", [["ask", "对话中询问"], ["deny", "直接拒绝"]])), row("credential", "凭据 Shell", "凭据提取并外传始终硬阻断；此项控制其他可疑 Shell 操作。", select("security-credential-shell", "credentialShellPolicy", "strict", [["strict", "严格阻断"], ["ask", "对话中询问"]]))] })
				] }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityNotice", children: "固定保护：凭据外传、子代理访问敏感路径等硬阻断始终生效，不会被宽松设置覆盖。审批卡片会显示在当前对话输入区上方。" }),
				state.error && (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityError", role: "alert", children: state.error }),
				state.loading && (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityEmpty", children: "正在读取安全设置…" })
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
		const subagentCss = ".dshSub{display:flex;flex-direction:column;gap:18px;width:100%;max-width:720px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshSub h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshSub h3{margin:0;font-size:13px;font-weight:600;color:var(--dsw-alias-label-tertiary);text-transform:uppercase;letter-spacing:.04em}.dshSubHint{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshSubGroup{display:flex;flex-direction:column;gap:12px;padding:16px;border:1px solid var(--dsw-alias-border-subtle);border-radius:14px;background:var(--dsw-alias-bg-layer-1)}.dshSubGrid{display:grid;grid-template-columns:160px minmax(0,1fr);gap:12px 16px;align-items:center}.dshSubGrid>label{font-size:13px;color:var(--dsw-alias-label-secondary);text-align:right;line-height:20px}.dshSubGrid small{display:block;font-size:11px;color:var(--dsw-alias-label-tertiary);margin-top:3px;font-weight:400}.dshSub input,.dshSub select{box-sizing:border-box;width:100%;max-width:320px;border:1px solid var(--dsw-alias-border-subtle);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px;transition:border-color .15s}.dshSub input:focus,.dshSub select:focus{outline:none;border-color:var(--dsw-alias-border-emphasis)}.dshSub input[type=checkbox]{width:16px;height:16px;max-width:none;justify-self:start;cursor:pointer}.dshSub input[type=number]{max-width:160px}.dshSubError{color:var(--dsw-alias-state-error-primary);font-size:13px;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08))}.dshSubRow{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshSubBadge{font-size:11px;padding:2px 8px;border-radius:6px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary)}";
		if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='dsh-subagent-settings']")) { const tag = document.createElement("style"); tag.dataset.pluginCss = "dsh-subagent-settings"; tag.textContent = subagentCss; document.head.appendChild(tag); }
		const subagentFields = [
			{ field: "defaultProvider", label: "子智能体提供方", type: "text", placeholder: "留空继承当前会话", hint: "例如 spawn、fork；留空则使用当前会话路由" },
			{ field: "defaultModel", label: "子智能体模型", type: "text", placeholder: "留空继承父会话", hint: "独立模型 ID；覆盖父会话模型选择" },
			{ field: "defaultReasoningEffort", label: "子智能体推理强度", type: "select", options: [["", "（继承/未设置）"], ["off", "关闭"], ["minimal", "极简"], ["low", "低"], ["medium", "中"], ["high", "高"], ["max", "最高（xhigh）"]], hint: "应用于所有子智能体的 LLM 请求；单工具调用仍可覆盖" },
			{ field: "defaultMaxTokens", label: "子智能体输出上限", type: "number", placeholder: "0 = 不限制", hint: "每个子智能体单次模型响应的最大 token 数" },
			{ field: "maxTurns", label: "子智能体轮次上限", type: "number", hint: "单次子智能体运行最多多少轮工具调用" },
			{ field: "maxParallel", label: "并行子智能体", type: "number", hint: "单个工作流中允许并行运行的子智能体数" },
			{ field: "maxDepth", label: "子智能体嵌套深度", type: "number", placeholder: "0 = 不限制", hint: "子智能体可以继续派生的最大深度（0 表示不限制）" },
			{ field: "timeoutSeconds", label: "子智能体超时（秒）", type: "number", placeholder: "0 = 不限制", hint: "单次子智能体运行的总时长上限；0 表示不限制" },
			{ field: "toolCallMode", label: "工具调用呈现", type: "select", options: [["auto", "自动"], ["code", "代码块"], ["native", "原生"]], hint: "子智能体工具调用在轨迹中的呈现方式" },
			{ field: "serviceTier", label: "服务等级", type: "text", placeholder: "（无）", hint: "透传给支持 service_tier 的 Provider（OpenAI/Anthropic）" },
			{ field: "apiRetryCount", label: "API 重试次数", type: "number", hint: "子智能体调用 LLM 失败时的重试次数" }
		];
		function SubagentSection({ api }) {
			const [state, setState] = (0, react.useState)({ namespace: null, loading: true, error: null });
			const load = (0, react.useCallback)(async () => {
				try {
					const reply = await api.settings.describe({});
					if (!reply.result.ok) throw new Error(reply.result.error.message);
					const ns = reply.result.value.namespaces.find((n) => n.ns === "subagent") ?? null;
					setState({ namespace: ns, loading: false, error: null });
				} catch (cause) {
					setState((s) => ({ ...s, loading: false, error: cause instanceof Error ? cause.message : String(cause) }));
				}
			}, [api]);
			(0, react.useEffect)(() => { load(); }, [load]);
			const setField = async (field, value) => {
				const ns = state.namespace;
				if (!ns) return;
				const reply = await api.settings.mutate({ ns: "subagent", ops: [{ op: "set", path: [field], value }], expectedRevision: ns.revision });
				if (!reply.result.ok) {
					setState((s) => ({ ...s, error: reply.result.error.message }));
					await load();
					return;
				}
				setState((s) => ({ ...s, namespace: reply.result.value, error: null }));
			};
			if (state.loading) return (0, react_jsx_runtime.jsx)("section", { className: "dshSub", children: (0, react_jsx_runtime.jsx)("div", { className: "dshSubHint", children: "正在加载子智能体设置…" }) });
			const v = (field) => state.namespace?.value?.[field];
			return (0, react_jsx_runtime.jsxs)("section", { className: "dshSub", children: [
				(0, react_jsx_runtime.jsx)("h2", { children: "子智能体" }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSubHint", children: "为所有子智能体（spawn / fork / workflow）配置默认的提供方、模型、推理强度、轮次和超时。留空或 0 表示继承父会话。" }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSubGroup", children: (0, react_jsx_runtime.jsx)("h3", { children: "默认值与限制" }) }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSubGroup", children: (0, react_jsx_runtime.jsx)("div", { className: "dshSubGrid", children: subagentFields.map((f) => [
					(0, react_jsx_runtime.jsxs)("label", { htmlFor: "sub-" + f.field, children: [f.label, f.hint ? (0, react_jsx_runtime.jsx)("small", { children: f.hint }) : null] }, f.field + "-l"),
					f.type === "select"
						? (0, react_jsx_runtime.jsx)("select", { id: "sub-" + f.field, value: v(f.field) ?? "", onChange: (e) => setField(f.field, e.target.value), children: f.options.map(([val, label]) => (0, react_jsx_runtime.jsx)("option", { value: val, children: label }, val)) }, f.field)
						: (0, react_jsx_runtime.jsx)("input", { id: "sub-" + f.field, type: f.type === "number" ? "number" : "text", inputMode: f.type === "number" ? "numeric" : void 0, step: f.type === "number" ? "1" : void 0, min: f.type === "number" ? "0" : void 0, value: v(f.field) ?? "", placeholder: f.placeholder, onChange: (e) => setField(f.field, f.type === "number" ? Number(e.target.value) : e.target.value) }, f.field)
				]).flat() }) }),
				state.error && (0, react_jsx_runtime.jsx)("div", { className: "dshSubError", role: "alert", children: state.error })
			] });
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
			"general.nav": "通用设置",
			"connection.error": "连接异常",
			"connection.retry": "立即重连",
			"connection.connecting": "连接中",
			"connection.connected": "连接成功",
			"connection.reconnect": "连接异常，点击立即重连",
			"connection.restart": "连接中，点击立即重连"
		};
		/** English dictionary, checked complete against the zh key set. */
		const en = {
			"trigger": "Settings",
			"title": "Settings",
			"close": "Close",
			"openDocument": "Open configuration file",
			"openDocument.error": "Could not open configuration file",
			"general.nav": "General",
			"connection.error": "Disconnected",
			"connection.retry": "Reconnect now",
			"connection.connecting": "Connecting",
			"connection.connected": "Connected",
			"connection.reconnect": "Disconnected, reconnect now",
			"connection.restart": "Connecting, restart now"
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
			const shellInjected = () => ({
				reconnect: () => {
					connection.reconnect();
				},
				t: ctx.locale.bind(NS),
				hooks: {
					connectionState: connection.state,
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
				}
			});
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
				id: "subagent",
				order: 22,
				label: "子智能体"
			}, () => (0, react_jsx_runtime.jsx)(SubagentSection, { api: connection.api })));
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