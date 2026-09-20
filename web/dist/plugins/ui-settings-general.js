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
		let react_dom = require("react-dom");
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
		const css$3 = "._7h7_Oq_trigger,.dshSidebarAction{box-sizing:border-box;cursor:pointer;width:calc(100% + 8px);height:34px;color:var(--dsw-alias-label-primary);background:0 0;border:none;border-radius:12px;flex:none;align-items:center;gap:8px;margin:4px -4px;padding:6px 2px 6px 10px;font-family:inherit;font-size:14px;line-height:22px;display:flex;overflow:hidden}._7h7_Oq_trigger:hover,.dshSidebarAction:hover{background:var(--dsw-alias-interactive-bg-hover)}._7h7_Oq_trigger._7h7_Oq_rail,.dshSidebarAction[data-rail=true]{border-radius:50%;justify-content:center;gap:0;width:36px;height:36px;margin:8px 0 10px;padding:0}._7h7_Oq_triggerLabel{white-space:nowrap;overflow:hidden}._7h7_Oq_overlay{z-index:1000;justify-content:center;align-items:center;display:flex;position:fixed;inset:0}._7h7_Oq_mask{background:var(--dsw-alias-bg-mask-1);backdrop-filter:var(--dsw-mask-blur);position:absolute;inset:0}._7h7_Oq_panel{z-index:1;background:var(--dsw-alias-bg-layer-2);width:800px;max-width:calc(100vw - 48px);height:min(800px,100vh - 48px);box-shadow:var(--dsw-shadow-lv3);--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2);--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2);border-radius:24px;display:flex;position:relative;overflow:hidden}._7h7_Oq_nav{box-sizing:border-box;flex-direction:column;flex:none;gap:18px;width:188px;padding:22px 12px 0;display:flex}._7h7_Oq_navTitle{color:var(--dsw-alias-label-primary);padding:0 12px;font-size:16px;font-weight:500;line-height:24px}._7h7_Oq_navList{flex-direction:column;gap:4px;display:flex}._7h7_Oq_navCell{box-sizing:border-box;cursor:pointer;height:40px;color:var(--dsw-alias-label-primary);text-align:left;background:0 0;border:none;border-radius:12px;align-items:center;gap:8px;padding:9px 16px 9px 12px;font-family:inherit;font-size:14px;font-weight:400;line-height:22px;display:flex}._7h7_Oq_navCell:hover{background:var(--dsw-specific-sidebar-nav-item-hover)}._7h7_Oq_navCell._7h7_Oq_active{background:var(--dsw-specific-sidebar-nav-item-active)}._7h7_Oq_navIcon{flex:none}._7h7_Oq_navLabel{white-space:nowrap;text-overflow:ellipsis;flex:1;min-width:0;overflow:hidden}._7h7_Oq_content{flex-direction:column;flex:1;min-width:0;display:flex}._7h7_Oq_header{box-sizing:border-box;flex:none;justify-content:space-between;align-items:flex-start;gap:8px;height:54px;padding:20px 14px 8px 10px;display:flex}._7h7_Oq_actions{justify-content:flex-end;align-items:center;gap:8px;min-width:0;margin-left:auto;display:flex}._7h7_Oq_close{cursor:pointer;width:28px;height:28px;color:var(--dsw-alias-label-primary);background:0 0;border:none;border-radius:28px;justify-content:center;align-items:center;padding:0;display:inline-flex}._7h7_Oq_close:hover{background:var(--dsw-alias-interactive-bg-hover)}._7h7_Oq_options{flex:1;min-height:0;padding:0 24px 24px;overflow-y:auto}._7h7_Oq_hiddenLabel{clip:rect(0 0 0 0);white-space:nowrap;width:1px;height:1px;position:absolute;overflow:hidden}";
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

		const mobileSettingsCss = "@media(max-width:768px){body ._7h7_Oq_panel{width:100%;max-width:100%;height:100%;height:100dvh;max-height:100dvh;border-radius:0;flex-direction:column}body ._7h7_Oq_nav{width:100%;padding:12px 8px 0;gap:10px;min-width:0}body ._7h7_Oq_navTitle{padding-right:48px}body ._7h7_Oq_navTitle{padding:0 48px 0 8px;min-height:32px;display:flex;align-items:center}body ._7h7_Oq_navList{flex-direction:row;gap:4px;overflow-x:auto;overscroll-behavior-x:contain;padding-bottom:8px;scrollbar-width:thin}body ._7h7_Oq_navCell{flex:none;min-height:40px;padding:8px 12px}body ._7h7_Oq_content{min-height:0}body ._7h7_Oq_header{height:40px}body ._7h7_Oq_header{height:40px;padding:2px 12px 4px;align-items:center}body ._7h7_Oq_close{position:absolute;right:8px;top:8px;width:40px;height:40px}body ._7h7_Oq_options{padding:0 12px max(20px,env(safe-area-inset-bottom));overflow-x:hidden;overscroll-behavior:contain}body ._7h7_Oq_options :where(section ,body article ,body fieldset ,body label ,body form){min-width:0;max-width:100%;box-sizing:border-box}body ._7h7_Oq_options :where(input ,body textarea ,body select){min-width:0;max-width:100%;box-sizing:border-box}body ._7h7_Oq_options :where(input:not([type=checkbox]):not([type=radio]) ,body select){min-height:40px}body ._7h7_Oq_options :where(button){min-height:36px;max-width:100%}body ._7h7_Oq_options :where(p ,body h2 ,body h3 ,body h4 ,body code){overflow-wrap:anywhere}body .feE0ya_themeCube{flex:1 1 100px;padding:16px 12px;border-radius:16px}body .dshMemory ,body .dshSubagents ,body .dshCaps{width:100%;max-width:100%;box-sizing:border-box}body .dshMemoryGrid{grid-template-columns:minmax(0,1fr);gap:6px}body .dshMemoryGrid>label{text-align:left}body .dshMemoryItemHead{flex-wrap:wrap}body .dshMemoryItemHead strong{flex:1 1 100%}body .dshMemoryItem p{overflow-wrap:anywhere}body .dshMemoryEditor ,body .dshCapsEditor{padding:12px}body .dshExperienceFilters input{min-width:0;flex:1 1 100%}body .dshExperienceFilters select{max-width:100%;flex:1}body .dshExperienceFacts{grid-template-columns:1fr;gap:2px}body .dshExperienceFacts dd{margin-bottom:5px}body .-\\35 oaCW_rowHead ,body .-\\35 oaCW_editorHeader ,body .-\\35 oaCW_modelListHead{flex-wrap:wrap}body .-\\35 oaCW_rowActions{flex-wrap:wrap}body .-\\35 oaCW_rowActions{flex-wrap:wrap;margin-left:0;gap:4px}body .-\\35 oaCW_rowName{overflow-wrap:anywhere}body .-\\35 oaCW_rowIdentity{flex-wrap:wrap}body .-\\35 oaCW_modelRow{grid-template-columns:minmax(0,1fr) 56px 36px 36px}body .-\\35 oaCW_modelRow>input:first-child{grid-column:1/-1}body .-\\35 oaCW_modelAdvanced ,body .dshModelFields{grid-template-columns:minmax(0,1fr)}body .-\\35 oaCW_editorActions{flex-wrap:wrap}body .-\\35 oaCW_addButton{min-width:0;flex-basis:100%}body .dshModelToolbar input{min-width:0;flex-basis:100%}body .dshModelVisibility{min-height:36px;white-space:nowrap}body .dshCapsBar{min-width:0}body .dshCaps input[type=search]{min-width:0;flex-basis:100%}body .a2nWpa_search input{box-sizing:border-box;min-width:0}body .a2nWpa_cards{grid-template-columns:minmax(0,1fr)}body .a2nWpa_groupTitleRow{flex-wrap:wrap}body .a2nWpa_switcher{max-width:100%}body .a2nWpa_headerEnd{max-width:100%}body .a2nWpa_switcherLabel{min-width:0;max-width:100%}\n}";
		if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='mobileSettingsCss']")) { const style = document.createElement("style"); style.dataset.pluginCss = "mobileSettingsCss"; style.textContent = mobileSettingsCss; document.head.appendChild(style); }
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
            if (id === "agent-teams") id = "subagent";
            const paths={
                "runtime-paths":["M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1Z"],
                memory:["M8 3a4 4 0 0 0-4 4v2a4 4 0 0 0 0 6v2a4 4 0 0 0 8 0V7a4 4 0 0 0-4-4Zm8 0a4 4 0 0 1 4 4v2a4 4 0 0 1 0 6v2a4 4 0 0 1-8 0V7a4 4 0 0 1 4-4Z","M4 9h3m10 0h3M6 16h2m8 0h2"],
                subagent:["M9 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm8-7a3 3 0 0 1 0 6M2 21v-2a6 6 0 0 1 12 0v2m4-7a5 5 0 0 1 4 5v2"],
                security:["M12 3 3 7v6c0 4 5 7 9 9 4-2 9-5 9-9V7Z","m8 12 3 3 5-6"],
                capabilities:["M8 3v5H3v13h18V8h-5V3Z","M8 8h8M8 13h8M8 17h5"],
                "archived-sessions":["M3 3h18v5H3Zm2 5v13h14V8M9 12h6"],
                trash:["M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7"]
            }[id];
            if(paths)return (0,react_jsx_runtime.jsx)("svg",{className:SettingsRoot_module_css_default.navIcon,width:18,height:18,viewBox:"0 0 24 24",fill:"none",stroke:"currentColor",strokeWidth:1.7,strokeLinecap:"round",strokeLinejoin:"round","aria-hidden":true,children:paths.map((d,index)=>(0,react_jsx_runtime.jsx)("path",{d},index))});
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
        function SettingsPanel({ rows, renderSlot, activeId, onSelect, onClose, t }) {
            const active = rows.find((r) => r.id === activeId)?.id ?? rows[0]?.id;
            const titleId = (0, react.useId)();
            const [confirmClose,setConfirmClose]=(0,react.useState)(false);
            const panelRef=(0,react.useRef)(null);
            const requestClose=()=>{if(window.__DSH_SETTINGS_DIRTY__){setConfirmClose(true);return;}onClose()};
            (0, react.useEffect)(() => {
                const onKeyDown = (e) => {
                    if (e.key === "Escape") {e.preventDefault();if(confirmClose)setConfirmClose(false);else requestClose();return;}
                    if(e.key!=="Tab")return;
                    const panel=panelRef.current;if(!panel)return;
                    const focusable=[...panel.querySelectorAll("button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),a[href]")].filter(node=>node.offsetParent!==null);
                    if(!focusable.length)return;
                    const first=focusable[0],last=focusable[focusable.length-1];
                    if(e.shiftKey&&document.activeElement===first){e.preventDefault();last.focus();}
                    else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first.focus();}
                };
                document.addEventListener("keydown", onKeyDown);
                return () => {
                    document.removeEventListener("keydown", onKeyDown);
                };
            }, [onClose,confirmClose]);
			const closeButton = (0, react.useRef)(null);
			(0, react.useEffect)(() => {
				closeButton.current?.focus();
			}, []);
			return (0, react_dom.createPortal)((0, react_jsx_runtime.jsxs)("div", {
				className: SettingsRoot_module_css_default.overlay,
				role: "presentation",
				children: [(0, react_jsx_runtime.jsx)("div", {
					className: SettingsRoot_module_css_default.mask,
					"aria-hidden": "true",
                    onClick: requestClose
                }), (0, react_jsx_runtime.jsxs)("div", {
                    ref: panelRef,
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
									if(window.__DSH_SETTINGS_DIRTY__){setConfirmClose(true);return;}
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
                                onClick: requestClose,
								children: [(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconCloseOutline16, { size: 14 }), (0, react_jsx_runtime.jsx)("span", {
									className: SettingsRoot_module_css_default.hiddenLabel,
									children: renderSlot("settings.close", {})
								})]
							})]
						}), (0, react_jsx_runtime.jsx)("div", {
							className: SettingsRoot_module_css_default.options,
                            children: active !== void 0 && renderSlot("settings.section", { close: requestClose }, { only: active })
                        }),
                        confirmClose&&(0,react_jsx_runtime.jsxs)("div",{className:"dshUnsavedDialog",role:"alertdialog","aria-modal":"true","aria-labelledby":`${titleId}-unsaved`,children:[
                            (0,react_jsx_runtime.jsx)("strong",{id:`${titleId}-unsaved`,children:t?.("unsaved.title")??"未保存的修改"}),
                            (0,react_jsx_runtime.jsx)("p",{children:t?.("unsaved.description")??"当前页面有未保存的修改。"}),
                            (0,react_jsx_runtime.jsxs)("div",{className:"dshUnsavedActions",children:[
                                (0,react_jsx_runtime.jsx)("button",{type:"button",onClick:()=>setConfirmClose(false),children:t?.("unsaved.continue")??"继续编辑"}),
                                (0,react_jsx_runtime.jsx)("button",{type:"button",className:"dshUnsavedDiscard",onClick:()=>{setConfirmClose(false);onClose()},children:t?.("unsaved.discard")??"放弃修改并关闭"})
                            ]})
                        ]})]
					})]
				})]
			}), document.body);
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
			const [activeId, setActiveId] = (0, react.useState)(() => { try { return window.sessionStorage.getItem("dsh-settings-section") || void 0; } catch { return void 0; } });
			const [completedOnboarding, setCompletedOnboarding] = (0, react.useState)(() => /* @__PURE__ */ new Set());
			const [showRecovery, setShowRecovery] = (0, react.useState)(false);
			const triggerButton = (0, react.useRef)(null);
			const wasOpen = (0, react.useRef)(open);
			const close = (0, react.useCallback)(() => {
				setOpen(false);
			}, []);
			(0, react.useEffect)(() => {
				if (wasOpen.current && !open) triggerButton.current?.focus();
				wasOpen.current = open;
			}, [open]);
			const openSection = (0, react.useCallback)((id) => {
				setActiveId(id);
				try { window.sessionStorage.setItem("dsh-settings-section", id); } catch {}
				setOpen(true);
			}, []);
			const selectSection=(0,react.useCallback)(id=>{setActiveId(id);try{window.sessionStorage.setItem("dsh-settings-section",id)}catch{}},[]);
			const rows = useSections((s) => s);
            (0,react.useEffect)(()=>{const open=event=>{if(rows.some(row=>row.id===event.detail?.section))openSection(event.detail.section)};window.addEventListener("dsh-open-settings",open);return()=>window.removeEventListener("dsh-open-settings",open)},[rows,openSection]);
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
					onSelect: selectSection,
					onClose: close,
                    t
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
		function DirectoryPickerPreference({ t }) {
			const [mode, setMode] = (0, react.useState)(() => {
				try { return localStorage.getItem("dsh.directory-picker-mode") === "native" ? "native" : "browse"; } catch { return "browse"; }
			});
			const update = (value) => { setMode(value); try { localStorage.setItem("dsh.directory-picker-mode", value); } catch {} };
			const [open, setOpen] = (0, react.useState)(false);
			return (0, react_jsx_runtime.jsxs)("div", { className: "dshDirectoryPickerRow", "data-directory-picker": true, children: [
				(0, react_jsx_runtime.jsxs)("div", { className: "dshDirectoryPickerText", children: [
					(0, react_jsx_runtime.jsx)("div", { className: "dshDirectoryPickerTitle", children: t("directoryPicker.title") }),
					(0, react_jsx_runtime.jsx)("div", { className: "dshDirectoryPickerDesc", children: t("directoryPicker.hint") })
				] }),
				(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Menu, { open, onClose: () => setOpen(false), items: ["browse", "native"].map(id => ({ id, label: t("directoryPicker." + id) })), selectedId: mode, onSelect: id => { setOpen(false); update(id); }, align: "end", portal: true,
					anchor: (0, react_jsx_runtime.jsxs)("button", { type: "button", className: "dshDirectoryPickerSelector", "aria-label": t("directoryPicker.label"), "aria-haspopup": "menu", "aria-expanded": open, onClick: () => setOpen(value => !value), children: [t("directoryPicker." + mode), (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconChevronDownOutline14, {})] })
				})
			] });
		}
		function GeneralSection({ renderSlot, t }) {
			return (0, react_jsx_runtime.jsx)("div", {
				className: GeneralSection_module_css_default.section,
				children: (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [renderSlot("settings.general.item", {}), (0, react_jsx_runtime.jsx)(DirectoryPickerPreference, { t })] })
			});
		}
		const securityCss = ".dshDirectoryPickerRow{border-bottom:1px solid var(--dsw-alias-border-l2);align-items:center;gap:8px;padding:16px 0;display:flex}.dshDirectoryPickerText{flex-direction:column;flex:1;gap:4px;min-width:0;padding-right:48px;display:flex}.dshDirectoryPickerTitle{color:var(--dsw-alias-label-primary);font-size:14px;font-weight:400;line-height:22px}.dshDirectoryPickerDesc{color:var(--dsw-alias-label-tertiary);font-size:12px;font-weight:400;line-height:18px}.dshDirectoryPickerSelector{background:var(--dsw-alias-bg-module-platform);height:36px;font:inherit;color:var(--dsw-alias-label-primary);cursor:pointer;border:none;border-radius:18px;align-items:center;gap:12px;padding:0 14px;font-size:14px;line-height:22px;display:inline-flex}.dshDirectoryPickerSelector:hover{background:var(--dsw-alias-interactive-bg-hover)}.dshDirectoryPickerRow+.dshSecurity{margin-top:20px}.dshSecurity{box-sizing:border-box;display:flex;flex-direction:column;gap:16px;width:100%;max-width:640px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshSecurityHeader{display:flex;flex-direction:column;gap:4px}.dshSecurity h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshSecurityIntro{font-size:12px;line-height:19px;color:var(--dsw-alias-label-tertiary)}.dshSecurityGroup{display:flex;flex-direction:column;border:1px solid var(--dsw-alias-border-l2);border-radius:14px;background:var(--dsw-alias-bg-layer-1);overflow:hidden}.dshSecurityGroupTitle{padding:12px 16px 8px;font-size:12px;font-weight:600;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshSecurityRow{display:grid;grid-template-columns:minmax(150px,1fr) minmax(180px,240px);align-items:center;gap:16px;padding:12px 16px;border-top:1px solid var(--dsw-alias-border-l2)}.dshSecurityRow:first-of-type{border-top:none}.dshSecurityRowText{min-width:0}.dshSecurityRowLabel{font-size:13px;font-weight:500;line-height:20px}.dshSecurityRowHint{margin-top:2px;font-size:11px;line-height:17px;color:var(--dsw-alias-label-tertiary)}.dshSecurityRow select,.dshSecurityRow input{box-sizing:border-box;width:100%;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-base);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px}.dshSecurityRow input[type=number]{max-width:120px;justify-self:end}.dshSecurityNotice{padding:12px 14px;border-radius:12px;background:var(--dsw-alias-bg-layer-1);font-size:12px;line-height:19px;color:var(--dsw-alias-label-secondary)}.dshSecurityError{padding:9px 12px;border-radius:9px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08));color:var(--dsw-alias-state-error-primary);font-size:13px}.dshSecurityEmpty{font-size:13px;color:var(--dsw-alias-label-tertiary);padding:8px 0}@media(max-width:640px){.dshSecurityRow{grid-template-columns:1fr;gap:8px}.dshSecurityRow input[type=number]{justify-self:start}}";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css='dsh-security-shield']") === null) {
			const tag = document.createElement("style");
			tag.dataset.pluginCss = "dsh-security-shield";
			tag.textContent = securityCss;
			document.head.appendChild(tag);
		}
		function SecuritySection({ api }) {
			const [state, setState] = (0, react.useState)({ loading: true, error: null, namespace: null, timeout: 300, preset: "workspace-write" });
			const load = (0, react.useCallback)(async () => {
				try {
					const settingsReply = await api.settings.describe({});
					if (!settingsReply.result.ok) throw new Error(settingsReply.result.error.message);
					const namespaces = settingsReply.result.value.namespaces ?? [];
					const securityNamespace = namespaces.find((entry) => entry.ns === "security") ?? null;
					const security = securityNamespace?.value ?? {};
					const permission = namespaces.find((entry) => entry.ns === "permission")?.value ?? {};
					setState({ loading: false, error: null, namespace: securityNamespace, timeout: security.approvalTimeoutSeconds ?? 300, preset: permission.defaultPreset ?? "workspace-write" });
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
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityGroup", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityGroupTitle", children: "审批行为" }), row("timeout", "审批超时", "等待对话中确认的最长时间。超时后按无人确认策略处理。", (0, react_jsx_runtime.jsx)("input", { id: "security-timeout", type: "number", min: 5, max: 300, step: 1, value: securityValue("approvalTimeoutSeconds", 300), onChange: (event) => setField("approvalTimeoutSeconds", Number(event.target.value)) })), row("unattended", "无人确认", "没有可用浏览器、断线或超时后的默认处理。", select("security-unattended", "unattendedPolicy", "deny", [["deny", "拒绝（推荐）"], ["allow-safe-only", "仅允许安全操作"], ["allow-all", "允许可审批操作"]]))] }),
					(0, react_jsx_runtime.jsxs)("div", { className: "dshSecurityGroup", children: [(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityGroupTitle", children: "工具与路径" }), row("risk", "破坏性命令", "删除、重置、终止进程等高风险命令。", select("security-risk", "riskToolPolicy", "ask", [["ask", "对话中询问"], ["deny", "直接拒绝"]])), row("outside", "工作区外写入", "控制对当前工作区之外文件的修改。", select("security-outside-write", "outsideWritePolicy", "ask-directory", [["ask-directory", "按目录询问，可记忆"], ["ask-every-time", "每次询问"], ["deny", "直接拒绝"]])), row("sensitive", "敏感路径读取", ".env、SSH、云凭据等敏感位置；授权不会被记忆。", select("security-sensitive-read", "sensitiveReadPolicy", "ask", [["ask", "对话中询问"], ["deny", "直接拒绝"]])), row("credential", "凭据 Shell", "凭据提取并外传始终硬阻断；此项控制其他可疑 Shell 操作。", select("security-credential-shell", "credentialShellPolicy", "strict", [["strict", "严格阻断"], ["ask", "对话中询问"]]))] })
				] }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSecurityNotice", children: "固定保护：凭据外传、子代理访问敏感路径等硬阻断始终生效，不会被宽松设置覆盖。审批卡片会显示在当前对话输入区上方。" }),
				state.error && (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityError", role: "alert", children: state.error }),
				state.loading && (0, react_jsx_runtime.jsx)("div", { className: "dshSecurityEmpty", children: "正在读取安全设置…" })
			] });
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-settings-general\src\client\SettingsDocumentAction.module.css.mjs
		const memoryCss = ".dshMemory{box-sizing:border-box;display:flex;flex-direction:column;gap:20px;width:100%;max-width:680px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshMemory h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshMemory h2+.dshMemoryHint,.dshMemory h2+.dshMemoryGrid{margin-top:-8px}.dshMemoryGrid{display:grid;grid-template-columns:120px minmax(0,1fr);gap:14px 16px;align-items:center}.dshMemoryGrid>label{font-size:13px;color:var(--dsw-alias-label-secondary);text-align:right;line-height:20px}.dshMemory input,.dshMemory select,.dshMemory textarea{box-sizing:border-box;width:100%;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px;transition:border-color .15s}.dshMemory input:focus,.dshMemory select:focus,.dshMemory textarea:focus{outline:none;border-color:var(--dsw-alias-border-l3)}.dshMemory input[type=checkbox]{width:auto;width:16px;height:16px;cursor:pointer;justify-self:start}.dshMemory input[type=number]{max-width:200px}.dshMemory input:disabled{opacity:.6;cursor:not-allowed}.dshMemory textarea{min-height:84px;resize:vertical}.dshMemoryToolbar{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshMemoryToolbar select{width:auto;min-width:130px}.dshMemoryToolbar label{display:inline-flex;align-items:center;gap:5px;font-size:13px;color:var(--dsw-alias-label-secondary);cursor:pointer}.dshMemoryToolbar label input[type=checkbox]{width:auto;width:15px;height:15px}.dshMemory button{border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:6px 14px;font-size:13px;line-height:20px;cursor:pointer;transition:background .15s,border-color .15s}.dshMemory button:hover{background:var(--dsw-alias-interactive-bg-hover);border-color:var(--dsw-alias-border-l3)}.dshMemory button:active{transform:translateY(1px)}.dshMemoryList{display:flex;flex-direction:column;gap:10px}.dshMemoryItem{border:1px solid var(--dsw-alias-border-l2);border-radius:12px;padding:14px 16px;display:flex;flex-direction:column;gap:10px;background:var(--dsw-alias-bg-layer-1)}.dshMemoryItemHead{display:flex;gap:10px;align-items:center}.dshMemoryItemHead strong{flex:1;font-size:14px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dshMemoryItemHead .dshMemoryBadge{font-size:11px;padding:2px 8px;border-radius:6px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary);white-space:nowrap}.dshMemoryItemHead button{padding:4px 10px;font-size:12px}.dshMemoryItem p{margin:0;font-size:13px;line-height:20px;white-space:pre-wrap;color:var(--dsw-alias-label-secondary)}.dshMemory .dshMemoryItem>input,.dshMemory .dshMemoryItem>textarea{margin:0}.dshMemoryError{color:var(--dsw-alias-state-error-primary);font-size:13px;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08))}.dshMemoryHint{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshMemoryDivider{height:1px;background:var(--dsw-alias-border-l2);margin:4px 0}";
		if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='dsh-memory-settings']")) { const tag = document.createElement("style"); tag.dataset.pluginCss = "dsh-memory-settings"; tag.textContent = memoryCss; document.head.appendChild(tag); }
        const experienceCss = ".dshExperiencePanel{display:flex;flex-direction:column;gap:12px;padding-top:16px;border-top:1px solid var(--dsw-alias-border-l2);min-width:0}.dshExperiencePanel h3{font-size:15px;line-height:22px;font-weight:500;margin:0}.dshExperienceHead{display:flex;align-items:center;gap:10px;flex-wrap:wrap}.dshExperienceHead h3{margin-right:auto}.dshExperienceHead label{display:inline-flex;align-items:center;gap:6px;font-size:12px}.dshExperiencePanel .dshMemoryItem{border-color:var(--dsw-alias-border-l2)}.dshExperienceFilters{display:flex;gap:8px;flex-wrap:wrap}.dshExperienceFilters input{flex:1;min-width:160px}.dshExperienceFilters select{width:auto;max-width:200px}.dshExperienceFacts{display:grid;grid-template-columns:112px minmax(0,1fr);gap:5px 10px;margin:0;font-size:12px;line-height:19px}.dshExperienceFacts dt{color:var(--dsw-alias-label-tertiary)}.dshExperienceFacts dd{margin:0;overflow-wrap:anywhere}.dshExperiencePanel pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:150px;overflow:auto;margin:0;font:12px/19px var(--ds-font-family-code,monospace);color:var(--dsw-alias-label-secondary)}.dshExperiencePanel details summary{font-size:12px;color:var(--dsw-alias-label-tertiary);cursor:pointer}.dshExperiencePanel details pre{margin-top:6px}.dshExperienceMatch{border-left:2px solid var(--dsw-alias-state-business-primary);padding-left:9px}.dshExperiencePanel textarea{min-height:90px}.dshExperiencePanel button:disabled{opacity:.45;cursor:default}.dshExperiencePanel[data-compact=true]{max-width:none;padding-bottom:0}.dshExperienceStatus{font-size:11px;line-height:17px;border:1px solid var(--dsw-alias-border-l2);border-radius:6px;padding:1px 7px;color:var(--dsw-alias-label-secondary)}.dshExperienceConfirm{display:flex;flex-direction:column;gap:8px;border-top:1px solid var(--dsw-alias-border-l2);padding-top:10px}@media(max-width:480px){.dshExperienceFacts{grid-template-columns:1fr;gap:2px}.dshExperienceFacts dd{margin-bottom:5px}.dshExperienceFilters select{max-width:100%}}";
        if(typeof document!=="undefined"&&!document.querySelector("style[data-plugin-css='dsh-experience-settings']")){const tag=document.createElement("style");tag.dataset.pluginCss="dsh-experience-settings";tag.textContent=experienceCss;document.head.appendChild(tag)}
        const experienceUnwrap=reply=>{if(!reply?.result?.ok)throw new Error(reply?.result?.error?.message||"自动经验请求失败");return reply.result.value};
        function experienceList(value){if(!value||typeof value.enabled!=="boolean"||typeof value.memoryEnabled!=="boolean"||typeof value.effectiveEnabled!=="boolean"||!Number.isSafeInteger(value.revision)||!Array.isArray(value.items)||!Number.isSafeInteger(value.total))throw new Error("自动经验目录返回的数据不完整");return value}
        const experienceDate=value=>typeof value==="number"&&Number.isFinite(value)&&value>0?new Date(value).toLocaleString():"未记录";
        const experienceCount=value=>Number.isSafeInteger(value)&&value>=0?String(value):"未提供";
        const experienceCategory=category=>({feedback:"用户反馈",authentication:"账号与访问权限","rate-limit":"服务限流",provider:"模型请求",arguments:"工具参数",parameters:"工具参数","filesystem-observation":"文件观察","tool-availability":"工具可用性",runtime:"运行环境",preflight:"工具前置检查","tool-error":"工具执行错误","request-format":"接入参数与格式",transport:"连接与响应传输","sandbox-runtime":"沙箱初始化",permissions:"目录访问权限",timeout:"执行超时","command-exit":"命令退出状态","filesystem-target":"文件目标与搜索","remote-runtime":"远程连接与环境"})[category]||"经验记录";
        const experienceControlCodes=new Set(["ABORTED","ABORTED_BEFORE_DISPATCH","CANCELLED","CANCELED","USER_APPROVAL_DENIED","USER_APPROVAL_CANCELLED","USER_APPROVAL_TIMED_OUT","USER_APPROVAL_UNAVAILABLE","COMPUTER_USE_MANUAL_CONTROL","COMPUTER_USE_ABORTED","COMPUTER_USE_HUMAN_REQUIRED","COMPUTER_USE_SESSION_NOT_FOUND","COMPUTER_USE_DEVICE_BUSY","COMPUTER_USE_SESSION_LIMIT","COMPUTER_USE_DESKTOP_DISCONNECTED","COMPUTER_USE_BUSY","COMPUTER_USE_FRAME_PENDING","COMPUTER_USE_CAPTURE_INTERRUPTED"]);
        function experienceAssessment(entry){
            const code=String(entry.code||"").toUpperCase();
            if(experienceControlCodes.has(code))return{kind:"control",label:"控制与连接状态",confirm:false,note:"这是暂停、审批或连接状态，不是模型输出错误；在工作台处理即可，无需逐条确认记忆。历史记录保留用于排查，不作为长期经验。"};
            if(entry.disposition==="diagnostic")return{kind:"diagnostic",label:"运行与服务诊断",confirm:false,note:"此记录用于排障，不作为后续模型的长期修正建议；先核对执行环境、工具实现和当前服务状态。"};
            if(entry.source==="provider")return{kind:"service",label:"服务与传输诊断",confirm:false,note:"请求链路、账号、限流或服务失败不等于模型回答错误；应检查服务状态和 Agent 接入实现，无需靠手动记忆解决。"};
            if(["TOOL_INPUT_INVALID","INVALID_ARGUMENTS","INVALID_INPUT","VALIDATION_ERROR","COMPUTER_USE_INVALID_ARGUMENT"].includes(code)||entry.ruleId==="tool-input-schema")return{kind:"arguments",label:"调用参数与工具契约",confirm:true,note:"参数校验失败需要对照原始调用和当前工具定义；也可能是校验器或适配器实现问题，不能仅凭错误代码归责于模型。"};
            if(entry.source==="feedback")return{kind:"feedback",label:"输出反馈待核对",confirm:true,note:"用户反馈需要结合具体回答核对；可补充已经验证的纠正经验，不自动把未经核实的评价写成长期规则。"};
            return{kind:"unresolved",label:"执行原因待定位",confirm:true,note:"需要结合实际调用、工具实现与运行环境定位；当时使用的模型仅用于追溯，不代表错误责任。"};
        }
        function experienceStatus(entry){const assessment=experienceAssessment(entry);if(assessment.kind==="control")return"运行状态";if(entry.status==="pending")return assessment.kind==="service"?"服务诊断":"待验证";if(entry.status==="verified")return entry.verification==="recovered"?"已验证 · 工具恢复":entry.verification==="user-confirmed"?"已确认 · 用户确认":"已验证";return"状态未提供"}
        const experienceMatchReason=(reason,toolSource)=>({"same-workspace-provider-model":"工作区、提供方和模型一致","same-workspace-tool":toolSource==="last-request"?"工作区与最近请求的工具快照匹配":toolSource==="unavailable"?"工作区一致，工具状态尚未确认":"工作区一致且当前工具可用","same-workspace-unavailable-tool":"工作区一致，用于提示此前不可用的工具"})[reason]||"满足当前匹配条件";
        const experienceExcludedReason=reason=>({"unrecognized-rule":"当前版本未识别此复用规则","tool-not-visible":"当前工具列表未提供此工具","context-budget":"达到条目数或上下文字符预算"})[reason]||"未满足当前匹配条件";
        function experienceWorkspace(entry,compact=false){const label=typeof entry.workspaceLabel==="string"?entry.workspaceLabel.trim():"";return label?(globalThis.__DSH_FILE_ACTIONS__?.displayPath(label)??label):compact?"当前工作区":entry.workspaceKey?`工作区 #${entry.workspaceKey.slice(0,12)}`:"工作区未记录"}
        function experienceReuse(entry,enabled,matched,excluded){const assessment=experienceAssessment(entry);if(assessment.kind==="diagnostic")return"保留用于排障，不注入后续模型上下文，也不要求手动确认记忆。";if(assessment.kind==="control")return"运行状态不进入长期经验，无需确认修正建议。";if(!enabled)return"自动捕获与复用已暂停，已有记录保留可查。";if(!entry.enabled)return"此条已停用，不参与经验复用。";if(entry.status!=="verified"){if(assessment.kind==="service")return"服务诊断不要求手动记忆；修复连接或服务后继续原任务。";return entry.source==="tool"&&["tool-input-schema","observe-before-write","unique-edit-target"].includes(entry.ruleId)?"修正后，系统可用同一任务内符合条件的工具成功来验证恢复，并自动用于后续匹配任务；不必手动确认。":"证据尚不足以自动形成通用经验。可以补充已验证的修正方法，但无需逐条手动确认。";}if(excluded)return`本次未纳入预览：${experienceExcludedReason(excluded.reason)}。`;if(matched)return`按当前条件已匹配：${experienceMatchReason(matched.matchReason,matched.toolSource)}。实际请求会重新检查匹配条件与预算。`;return entry.source==="tool"?"工作区与工具条件匹配时，可跨模型复用此建议；实际注入受条目数与上下文预算限制。":"仅在相同工作区及对应提供方、模型下复用；实际注入受条目数与上下文预算限制。"}

        function ExperiencePanel({api,sessionId,memoryEnabled}) {
            const h=react.createElement,compact=typeof sessionId==="string";
            const [report,setReport]=react.useState(null),[preview,setPreview]=react.useState(null),[query,setQuery]=react.useState(""),[status,setStatus]=react.useState(""),[workspace,setWorkspace]=react.useState(""),[workspaces,setWorkspaces]=react.useState([]),[expanded,setExpanded]=react.useState(false),[loading,setLoading]=react.useState(false),[readError,setReadError]=react.useState(null),[actionError,setActionError]=react.useState(null),[busy,setBusy]=react.useState(null),[notice,setNotice]=react.useState(null),[confirmation,setConfirmation]=react.useState(null),[removeId,setRemoveId]=react.useState(null);
            const mounted=react.useRef(true),generation=react.useRef(0),busyRef=react.useRef(false);
            const refresh=react.useCallback(async({quiet=false,force=false}={})=>{
                if(busyRef.current&&!force)return;
                const version=++generation.current;if(!quiet)setLoading(true);setReadError(null);
                try{
                    if(typeof api.memory?.learningList!=="function")throw new Error("当前主机尚未提供自动经验接口。");
                    let nextPreview=null,workspaceKey=workspace||undefined;
                    if(compact){
                        if(typeof api.memory.learningPreview!=="function")throw new Error("当前主机尚未提供经验匹配预览。");
                        nextPreview=experienceUnwrap(await api.memory.learningPreview({sessionId}));
                        if(!["next-request-preview","historical-context-preview"].includes(nextPreview?.mode)||typeof nextPreview.workspaceKey!=="string"||!nextPreview.workspaceKey||!Array.isArray(nextPreview.items)||typeof nextPreview.enabled!=="boolean")throw new Error("经验预览返回的数据不完整");
                        workspaceKey=nextPreview.workspaceKey;
                    }
                    const next=experienceList(experienceUnwrap(await api.memory.learningList({...workspaceKey?{workspaceKey}:{},...status?{status}:{},...query.trim()?{query:query.trim()}:{},limit:compact&&!expanded?6:100})));
                    if(!mounted.current||version!==generation.current)return;
                    setReport(next);setPreview(nextPreview);setWorkspaces(current=>{const known=new Map(current.map(item=>[item.key,item]));for(const entry of next.items)if(entry.workspaceKey)known.set(entry.workspaceKey,{key:entry.workspaceKey,label:experienceWorkspace(entry)});return[...known.values()]});
                }catch(error){if(mounted.current&&version===generation.current)setReadError(error instanceof Error?error.message:String(error))}
                finally{if(mounted.current&&version===generation.current)setLoading(false)}
            },[api,sessionId,compact,workspace,status,query,expanded,memoryEnabled]);
            react.useEffect(()=>{mounted.current=true;const initial=setTimeout(()=>refresh(),query?220:0),timer=setInterval(()=>{if(!document.hidden)refresh({quiet:true})},5000);return()=>{mounted.current=false;++generation.current;clearTimeout(initial);clearInterval(timer)}},[refresh]);
            const act=async(kind,payload)=>{
                if(busyRef.current)return;busyRef.current=true;setBusy(kind);setActionError(null);setNotice(null);++generation.current;
                try{if(typeof api.memory?.[kind]!=="function")throw new Error("当前主机不支持此经验操作");experienceUnwrap(await api.memory[kind](payload));if(!mounted.current)return;if(kind==="learningConfirm")setConfirmation(null);if(kind==="learningRemove")setRemoveId(null);setNotice(kind==="learningConfirm"?"已记录用户确认；这不等同于工具恢复验证。":kind==="learningRemove"?"经验已删除。":"设置已保存。");await refresh({quiet:true,force:true})}
                catch(error){if(mounted.current){setActionError(error instanceof Error?error.message:String(error));await refresh({quiet:true,force:true})}}
                finally{busyRef.current=false;if(mounted.current)setBusy(null)}
            };
            const masterEnabled=typeof memoryEnabled==="boolean"?memoryEnabled:report?.memoryEnabled;
            const selected=new Map((preview?.items??[]).map(item=>[item.id,{...item,toolSource:preview?.toolSource}])),excluded=new Map((preview?.excluded??[]).map(item=>[item.id,item]));
            const [showDiagnostics,setShowDiagnostics]=react.useState(false);
            const allRows=report?.items??[],diagnosticCount=report?.diagnosticTotal??allRows.filter(entry=>entry.disposition==="diagnostic").length;
            const rows=showDiagnostics?allRows:allRows.filter(entry=>entry.disposition!=="diagnostic");
            const button=(text,onClick,disabled=false)=>h("button",{type:"button",disabled:!!busy||disabled,onClick},text);
            const facts=(entry)=>{
                const count=Number.isSafeInteger(entry.occurrences)&&entry.occurrences>0?entry.occurrences:null;
                const modelNames=(entry.models??[]).map(model=>`${model.provider||"未记录提供方"} / ${model.model||"未记录模型"} × ${experienceCount(model.count)}`).join("；")||"未关联模型";
                return [["记录与重复",count===null?"未提供":`${count} 次记录 · 重复 ${Math.max(0,count-1)} 次`],["事件来源",({tool:"工具执行",provider:"服务请求",feedback:"用户反馈"})[entry.source]||"未记录"],["当时使用的模型",modelNames],["工作区",experienceWorkspace(entry,compact)],["首次 / 最近发生",`${experienceDate(entry.firstSeen)} / ${experienceDate(entry.lastSeen)}`],["最近恢复",experienceDate(entry.lastRecovered)],[entry.source==="tool"?"工具前应用次数":"模型请求前应用次数",experienceCount(entry.applicationCount)],["最近应用",`${experienceDate(entry.lastApplied)}${entry.lastApplicationOutcome?" · "+({preflight_blocked:"执行前拦截",advisory:"执行提示"}[entry.lastApplicationOutcome]||entry.lastApplicationOutcome):""}`]];
            };
            return h("section",{className:"dshMemory dshExperiencePanel","data-compact":compact||undefined,"data-experience-panel":compact?"context":"settings"},
                h("div",{className:"dshExperienceHead"},h("h3",null,"自动经验"),report&&!compact&&h("label",null,h("input",{type:"checkbox",checked:report.enabled,disabled:!!busy||masterEnabled!==true,"aria-label":"自动捕获与复用经验",onChange:event=>act("learningConfigure",{enabled:event.target.checked,expectedRevision:report.revision})}),"自动捕获与复用"),button("刷新",()=>refresh(),loading)),
                h("div",{className:"dshMemoryHint"},"当前回合可根据工具结果自行修正；跨任务经验在恢复验证后自动复用。运行状态、服务故障与模型反馈分别核对，Agent 实现缺陷应修复代码，无需靠你逐条确认记忆。"),
                report&&masterEnabled!==true&&h("div",{className:"dshMemoryHint",role:"status"},"持久记忆已关闭，自动捕获与复用暂停。已有记录仍可查询。"),
                report&&masterEnabled===true&&!report.effectiveEnabled&&h("div",{className:"dshMemoryHint",role:"status"},"自动捕获与经验复用已暂停，已有记录仍可查询。"),
                h("div",{className:"dshExperienceFilters"},h("input",{type:"search",value:query,disabled:!!busy,"aria-label":"查询自动经验",placeholder:"查询错误、工具、修正建议或来源模型",onChange:event=>setQuery(event.target.value)}),h("select",{value:status,disabled:!!busy,"aria-label":"经验验证状态",onChange:event=>setStatus(event.target.value)},h("option",{value:""},"全部状态"),h("option",{value:"pending"},"待验证"),h("option",{value:"verified"},"已验证 / 已确认")),!compact&&h("select",{value:workspace,disabled:!!busy,"aria-label":"经验工作区",onChange:event=>setWorkspace(event.target.value)},h("option",{value:""},"全部工作区"),workspaces.map(item=>h("option",{key:item.key,value:item.key},item.label)))),
                !report&&loading&&h("div",{className:"dshMemoryHint",role:"status"},"正在读取自动经验…"),
                report&&h("div",{className:"dshMemoryHint"},`共 ${experienceCount(report.total)} 条，显示 ${rows.length} 条${loading?" · 刷新中…":""}`),
                diagnosticCount>0&&button(showDiagnostics?"收起运行诊断":`查看运行诊断（${diagnosticCount}）`,()=>setShowDiagnostics(value=>!value)),
                preview&&h("div",{className:"dshMemoryHint dshExperienceMatch","data-experience-preview":true},
                    h("div",null,`${preview.mode==="historical-context-preview"?"历史快照匹配预览":"下次请求匹配预览"}：${preview.items.length} 条。${preview.enabled?"":"自动复用当前暂停。"}预览不增加实际应用次数，实际请求会重新匹配。`),
                    preview.notice&&h("div",{role:"status"},preview.notice),
                    h("div",null,`${preview.mode==="historical-context-preview"?"历史会话模型":"当前模型"}：${preview.provider||"未提供"} / ${preview.model||"未提供"} · ${experienceCount(preview.usedCharacters)} / ${experienceCount(preview.budget)} 字符 · 最多 ${experienceCount(preview.maxItems)} 条`),
                    h("details",null,h("summary",null,"查看具体复用建议与匹配依据"),preview.items.length?h("ol",null,preview.items.map(item=>h("li",{key:item.id},h("strong",null,item.tool||experienceCategory(item.category)),h("p",null,item.suggestion),h("div",null,experienceMatchReason(item.matchReason,preview.toolSource))))):h("p",null,"当前没有符合条件的已验证经验。")),
                    preview.text&&h("details",null,h("summary",null,"查看将加入上下文的内容"),h("pre",null,preview.text))),
                (readError||actionError||report?.lastError)&&h("div",{className:"dshMemoryError",role:"alert"},[actionError,readError,report?.lastError].filter(Boolean).join("\n")),notice&&h("div",{className:"dshMemoryHint",role:"status"},notice),
                report&&!rows.length&&h("div",{className:"dshMemoryHint"},query||status||workspace?"没有匹配的经验。":"尚无自动捕获记录。发生失败并被捕获后会在此显示。"),
                h("div",{className:"dshMemoryList"},rows.map(entry=>{
                    const assessment=experienceAssessment(entry),editing=confirmation?.id===entry.id,changed=editing&&confirmation.revision!==entry.revision;
                    return h("article",{key:entry.id,className:"dshMemoryItem","data-experience-id":entry.id},
                        h("div",{className:"dshMemoryItemHead"},h("strong",null,entry.tool||experienceCategory(entry.category)),h("span",{className:"dshExperienceStatus"},experienceStatus(entry)),assessment.kind!=="control"&&h("label",null,h("input",{type:"checkbox",checked:entry.enabled,disabled:!!busy,"aria-label":`${entry.enabled?"停用":"启用"}经验 ${entry.tool||entry.id}`,onChange:event=>act("learningToggle",{id:entry.id,enabled:event.target.checked,expectedRevision:entry.revision})}))),
                        h("div",{className:"dshMemoryHint","data-experience-assessment":assessment.kind},h("strong",null,assessment.label)," · ",assessment.note),
                        h("dl",{className:"dshExperienceFacts"},facts(entry).flatMap(([label,value])=>[h("dt",{key:label+"-label"},label),h("dd",{key:label},value)])),
                        entry.suggestion&&h("p",null,entry.suggestion),
                        h("details",null,h("summary",null,"错误类别与诊断"),h("div",{className:"dshMemoryHint"},`错误代码：${entry.code||"未提供"}`),h("pre",null,entry.message||"未记录诊断")),
                        entry.lastSessionId&&h("details",null,h("summary",null,"原始记录定位"),h("div",{className:"dshMemoryHint"},"任务：",h("code",null,entry.lastSessionId)),entry.lastCallId&&h("div",{className:"dshMemoryHint"},"调用：",h("code",null,entry.lastCallId))),
                        h("div",{className:"dshMemoryHint","data-experience-reuse":true},experienceReuse(entry,masterEnabled===true&&report.effectiveEnabled,selected.get(entry.id),excluded.get(entry.id))),
                        h("div",{className:"dshMemoryToolbar"},entry.status==="pending"&&assessment.confirm&&button("确认修正建议",()=>setConfirmation({id:entry.id,revision:entry.revision,suggestion:entry.suggestion||""})),button("删除",()=>setRemoveId(entry.id))),
                        editing&&h("div",{className:"dshExperienceConfirm"},h("div",{className:"dshMemoryHint"},"只确认已检查的修正建议；记录为用户确认，不代表已观察到工具恢复。"),h("textarea",{value:confirmation.suggestion,maxLength:1000,disabled:!!busy,"aria-label":"确认的修正建议",placeholder:"填写明确的修正步骤与适用条件",onChange:event=>setConfirmation({...confirmation,suggestion:event.target.value})}),changed&&h("div",{className:"dshMemoryHint"},"记录已更新，请核对上方最新证据。",button("已核对最新记录",()=>setConfirmation({...confirmation,revision:entry.revision}))),h("div",{className:"dshMemoryToolbar"},button("确认此建议",()=>act("learningConfirm",{id:entry.id,expectedRevision:confirmation.revision,confirmed:true,suggestion:confirmation.suggestion.trim()}),changed||!confirmation.suggestion.trim()||confirmation.suggestion.trim().length>1000),button("取消",()=>setConfirmation(null)))),
                        removeId===entry.id&&h("div",{className:"dshMemoryToolbar"},h("span",{className:"dshMemoryHint"},"删除此条经验记录？"),button("确认删除",()=>act("learningRemove",{id:entry.id,expectedRevision:entry.revision})),button("取消",()=>setRemoveId(null)))
                    );
                })),compact&&!expanded&&report&&report.total>rows.length&&button("展开当前工作区经验",()=>setExpanded(true))
            );
        }

		const memoryFields = [["enabled","持久记忆","checkbox"],["userProfileEnabled","用户画像","checkbox"],["memoryBudget","记忆预算","number"],["profileBudget","画像预算","number"],["provider","记忆提供方","fixed","仅内置"],["contextEngine","上下文引擎","fixed","Compressor"],["autoCompact","自动压缩","checkbox"],["compactThreshold","压缩阈值","number"],["compactTarget","压缩目标","number"],["protectRecentMessages","保护最近消息","number"]];
		function MemorySection({ api, renderSlot }) {
			const h=react.createElement;
			const [settings,setSettings]=react.useState(null),[entries,setEntries]=react.useState([]),[categories,setCategories]=react.useState([]),[scopes,setScopes]=react.useState(["default"]),[scope,setScope]=react.useState("default"),[category,setCategory]=react.useState(""),[draft,setDraft]=react.useState(null),[error,setError]=react.useState(null),[query,setQuery]=react.useState(""),[busy,setBusy]=react.useState(false),[removeId,setRemoveId]=react.useState(null);
			const generation=react.useRef(0),mounted=react.useRef(true);
			const unwrap=reply=>{if(!reply.result.ok)throw new Error(reply.result.error.message);return reply.result.value};
			const load=react.useCallback(async()=>{
				const turn=++generation.current;
				const [description,categoryReply,entryReply,roster,workspaceReply]=await Promise.all([api.settings.describe({}),api.memory.categories({}),api.memory.list({scope,...category?{category}:{}}),api.agentPresets.list({}),api.workspace?.list?.({})??Promise.resolve(null)]);
				const namespace=unwrap(description).namespaces.find(item=>item.ns==="memory")??null,groups=unwrap(categoryReply).categories??[],items=unwrap(entryReply).entries??[];
				if(!mounted.current||turn!==generation.current)return;
				setSettings(namespace);setCategories(groups);setEntries(items);
				if(roster.result.ok)setScopes([...new Set(["default",...(roster.result.value.presets??[]).map(item=>item.id),...items.map(item=>item.scope),...(workspaceReply?.result?.ok?workspaceReply.result.value.workspaces??[]:[]).map(w=>w.path)])]);
			},[api,scope,category]);
			react.useEffect(()=>{mounted.current=true;load().catch(cause=>setError(cause.message));return()=>{mounted.current=false;generation.current++}},[load]);
            react.useEffect(()=>{const refresh=()=>load().catch(cause=>setError(cause.message));window.addEventListener("dsh-memory-imported",refresh);return()=>window.removeEventListener("dsh-memory-imported",refresh)},[load]);
			const act=async action=>{if(busy)return;setBusy(true);setError(null);try{await action();await load()}catch(cause){if(mounted.current)setError(cause instanceof Error?cause.message:String(cause))}finally{if(mounted.current)setBusy(false)}};
			const setOption=(field,value)=>act(async()=>{if(!settings)return;const result=unwrap(await api.settings.mutate({ns:"memory",ops:[{op:"set",path:[field],value}],expectedRevision:settings.revision}));setSettings(result)});
			const write=entry=>api.memory.upsert({entry:{...entry,id:entry.id??"",revision:entry.revision??0},...entry.revision?{expectedRevision:entry.revision}:{}}).then(unwrap);
			const save=()=>act(async()=>{if(!draft)return;await write(draft);setDraft(null)});
			const remove=entry=>act(async()=>{unwrap(await api.memory.remove({id:entry.id,expectedRevision:entry.revision}));setRemoveId(null)});
			const visible=entries.filter(entry=>`${entry.title} ${entry.content}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
			const newEntry=(lesson=false)=>setDraft({scope,category:lesson?"known-error":category||"custom",title:"",content:lesson?"触发条件：\n错误做法：\n正确做法：\n验证方法：":"",enabled:true});
			return h("section",{className:"dshMemory"},
				h("h2",null,"记忆与上下文"),
				settings&&h("div",{className:"dshMemoryGrid"},memoryFields.flatMap(([field,label,type,fixed])=>[h("label",{key:field+"-l",htmlFor:"memory-"+field},label),type==="checkbox"?h("input",{key:field,id:"memory-"+field,type:"checkbox",disabled:busy,checked:Boolean(settings.value[field]),onChange:event=>setOption(field,event.target.checked)}):type==="fixed"?h("input",{key:field,id:"memory-"+field,value:fixed,disabled:true}):h("input",{key:field+":"+settings.revision,id:"memory-"+field,type:"number",disabled:busy,defaultValue:settings.value[field],step:field.includes("Threshold")||field.includes("Target")?"0.05":"1",onBlur:event=>{const value=Number(event.target.value);if(Number.isFinite(value)&&value!==settings.value[field])setOption(field,value)}})])),
				h(ExperiencePanel,{api,memoryEnabled:settings?.value?.enabled}),
				h("div",{className:"dshMemoryDivider"}),h("h2",null,"Agent 记忆管理"),
				h("div",{className:"dshMemoryHint"},"记忆在任务与模型之间保留；已知错误优先进入上下文。记录触发条件、修正步骤与验证方法，帮助后续任务避免重复错误。default 作用于全部预设；其他范围仅作用于对应预设。"),
				h("div",{className:"dshMemoryToolbar"},
					h("select",{"aria-label":"记忆作用范围",value:scope,onChange:e=>{setScope(e.target.value);setDraft(null)}},scopes.map(id=>h("option",{key:id,value:id},id))),
					h("select",{"aria-label":"记忆分类",value:category,onChange:e=>setCategory(e.target.value)},h("option",{value:""},"全部分类"),categories.map(item=>h("option",{key:item.id,value:item.id},item.label))),
					h("button",{disabled:busy,onClick:()=>newEntry(false)},"新增记忆"),h("button",{disabled:busy,onClick:()=>newEntry(true)},"记录错误经验")),
				h("input",{type:"search","aria-label":"搜索记忆",placeholder:"搜索标题、触发条件与修正方法",value:query,onChange:e=>setQuery(e.target.value)}),
				error&&h("div",{className:"dshMemoryError",role:"alert"},error),busy&&h("div",{className:"dshMemoryHint",role:"status"},"正在保存…"),
				draft&&h("div",{className:"dshMemoryItem"},
					h("input",{"aria-label":"记忆标题",value:draft.title,maxLength:200,placeholder:draft.category==="known-error"?"错误经验标题":"标题",onChange:e=>setDraft({...draft,title:e.target.value})}),
					h("textarea",{"aria-label":"记忆内容",value:draft.content,maxLength:20000,placeholder:"记忆内容",rows:draft.category==="known-error"?8:4,onChange:e=>setDraft({...draft,content:e.target.value})}),
					h("div",{className:"dshMemoryToolbar"},h("select",{"aria-label":"编辑记忆范围",value:draft.scope,onChange:e=>setDraft({...draft,scope:e.target.value})},scopes.map(id=>h("option",{key:id,value:id},id==="default"?"全局记忆":id))),h("select",{"aria-label":"编辑记忆分类",value:draft.category,onChange:e=>setDraft({...draft,category:e.target.value})},categories.map(item=>h("option",{key:item.id,value:item.id},item.label))),h("label",null,h("input",{type:"checkbox",checked:draft.enabled,onChange:e=>setDraft({...draft,enabled:e.target.checked})}),"启用"),h("button",{disabled:busy||!draft.title.trim()||!draft.content.trim(),onClick:save},"保存"),h("button",{disabled:busy,onClick:()=>setDraft(null)},"取消"))),
				h("div",{className:"dshMemoryList"},visible.length===0?h("div",{className:"dshMemoryHint"},query?"没有匹配的记忆":"暂无记忆"):visible.map(entry=>h("div",{key:entry.id,className:"dshMemoryItem"},
					h("div",{className:"dshMemoryItemHead"},h("strong",null,entry.title),h("span",{className:"dshMemoryBadge"},categories.find(item=>item.id===entry.category)?.label??entry.category),h("input",{type:"checkbox",role:"switch","aria-label":`${entry.enabled?"停用":"启用"}记忆 ${entry.title}`,checked:entry.enabled,disabled:busy,onChange:e=>act(()=>write({...entry,enabled:e.target.checked}))}),h("button",{disabled:busy,onClick:()=>setDraft({...entry})},"编辑"),h("button",{disabled:busy,onClick:()=>setRemoveId(entry.id)},"删除")),
					h("p",null,entry.content),!entry.enabled&&h("span",{className:"dshMemoryHint"},"已停用，不再注入上下文"),removeId===entry.id&&h("div",{className:"dshMemoryToolbar"},h("span",{className:"dshMemoryHint"},"确定删除此条记忆？"),h("button",{disabled:busy,onClick:()=>remove(entry)},"确认删除"),h("button",{onClick:()=>setRemoveId(null)},"取消"))))), renderSlot?.("settings.memory.import", {onImported:()=>load().catch(cause=>setError(cause.message))})
			);
		}
		const subagentCss = ".dshSub{display:flex;flex-direction:column;gap:18px;width:100%;max-width:720px;padding:4px 2px 28px;color:var(--dsw-alias-label-primary)}.dshSub h2{margin:0;font-size:18px;font-weight:600;line-height:26px}.dshSub h3{margin:0;font-size:13px;font-weight:600;color:var(--dsw-alias-label-tertiary);text-transform:uppercase;letter-spacing:.04em}.dshSubHint{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}.dshSubGroup{display:flex;flex-direction:column;gap:12px;padding:16px;border:1px solid var(--dsw-alias-border-l2);border-radius:14px;background:var(--dsw-alias-bg-layer-1)}.dshSubGrid{display:grid;grid-template-columns:160px minmax(0,1fr);gap:12px 16px;align-items:center}.dshSubGrid>label{font-size:13px;color:var(--dsw-alias-label-secondary);text-align:right;line-height:20px}.dshSubGrid small{display:block;font-size:11px;color:var(--dsw-alias-label-tertiary);margin-top:3px;font-weight:400}.dshSub input,.dshSub select{box-sizing:border-box;width:100%;max-width:320px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;padding:7px 10px;font:inherit;font-size:13px;line-height:20px;transition:border-color .15s}.dshSub input:focus,.dshSub select:focus{outline:none;border-color:var(--dsw-alias-border-l3)}.dshSub input[type=checkbox]{width:16px;height:16px;max-width:none;justify-self:start;cursor:pointer}.dshSub input[type=number]{max-width:160px}.dshSubError{color:var(--dsw-alias-state-error-primary);font-size:13px;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-state-error-bg,rgba(255,80,80,.08))}.dshSubRow{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshSubBadge{font-size:11px;padding:2px 8px;border-radius:6px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary)}";
		if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='dsh-subagent-settings']")) { const tag = document.createElement("style"); tag.dataset.pluginCss = "dsh-subagent-settings"; tag.textContent = subagentCss; document.head.appendChild(tag); }
		if (typeof document !== "undefined" && !document.querySelector("style[data-environment-controls]")) { const tag=document.createElement("style");tag.dataset.environmentControls="1";tag.textContent=" .dshEnvironment,.dshRemoteExecution{font-size:14px;line-height:1.6;min-width:0;gap:14px}.dshEnvironment p,.dshRemoteExecution p{margin:0;overflow-wrap:anywhere}.dshEnvironment label,.dshRemoteExecution label{min-width:0;overflow-wrap:anywhere}.dshEnvActions{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshEnvironment button,.dshRemoteExecution button{box-sizing:border-box;align-self:flex-start;max-width:100%;min-height:36px;padding:7px 12px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-primary);font:inherit;line-height:20px;cursor:pointer}.dshEnvironment button:disabled,.dshRemoteExecution button:disabled{opacity:.5;cursor:default}.dshEnvironment :is(button,input,select):focus-visible,.dshRemoteExecution :is(button,input,select):focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshEnvironment :is(input,select),.dshRemoteExecution :is(input,select){min-width:0}.dshEnvironment details summary{cursor:pointer;min-height:30px}.dshRemoteExecution button{margin-right:8px;margin-bottom:8px}";document.head.appendChild(tag); }
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
		async function environmentRequest(payload) {
			const response = await fetch("/__dsh-environment", { method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin", body: JSON.stringify(payload) });
			const value = await response.json();
			if (!response.ok || value.error) throw new Error(value.error || "环境请求失败");
			return value;
		}
		function EnvironmentSection({ sessionId }) {
			const h = react.createElement;
			const [state, setState] = react.useState(null);
			const [preferences, setPreferences] = react.useState({});
			const [scope, setScope] = react.useState(sessionId ? "session" : "global");
			const [cwd, setCwd] = react.useState("");
            const [cwdDraft, setCwdDraft] = react.useState("");
			const [error, setError] = react.useState("");
			const [busy, setBusy] = react.useState(false);
			const [result, setResult] = react.useState(null);
			const generation = react.useRef(0);
            const actionGeneration = react.useRef(0);
			const load = react.useCallback(async ({ preserveDraft = false } = {}) => {
				const token = ++generation.current;
				try { const value = await environmentRequest({ action: "describe", sessionId, cwd }); if (token !== generation.current) return; setState(previous => preserveDraft && previous ? { ...value, revision: previous.revision, preferences: previous.preferences } : value); if (!preserveDraft) setPreferences(value.preferences); setError(""); }
				catch (cause) { if (token === generation.current) setError(messageOf(cause)); }
			}, [sessionId, cwd]);
			react.useEffect(() => { actionGeneration.current++; setState(null); setPreferences({}); setResult(null); setBusy(false); setError(""); load(); return () => { generation.current++; actionGeneration.current++; }; }, [load]);
			const action = async (payload, replace = false) => {
                const token = ++actionGeneration.current;
                setBusy(true); setError("");
                try { const value = await environmentRequest({ sessionId, cwd, ...payload }); if (token !== actionGeneration.current) return; if (replace) { setState(value); setPreferences(value.preferences); } else { setResult(payload.action === "clearCache" ? null : value); await load({ preserveDraft: true }); } }
                catch (cause) { if (token === actionGeneration.current) setError(messageOf(cause)); }
                finally { if (token === actionGeneration.current) setBusy(false); }
            };
			const field = (key, value) => setPreferences(p => ({ ...p, [key]: value || null }));
			const labels = { ready: "当前环境可运行", located: "已定位，尚未启动验证", missing: "未找到", permission_denied: "权限不足", timed_out: "检查超时", error: "检查失败", unknown: "尚未验证" };
			const permissionLabels = { "read-only": "仅查看", "workspace-write": "工作区保护", "danger-full-access": "完全访问" };
			const row = (label, child) => h("label", { key: label, style: { display: "grid", gap: 6, margin: 0 } }, h("span", null, label), child);
			const button = (title, click, disabled = false) => h("button", { key: title, type: "button", disabled: busy || disabled, onClick: click }, title);
			const projectChanged = !sessionId && scope === "project" && cwdDraft.trim() !== cwd;
            const readProject = () => { const next = cwdDraft.trim(); if (next === cwd) return load(); setCwd(next); };
            const scopePicker = row("保存范围", h("select", { value: scope, onChange: e => { setScope(e.target.value); if (!sessionId && e.target.value !== "project") { setCwd(""); setCwdDraft(""); } }, disabled: busy }, [ ["global", "全局默认"], ["project", "项目默认"], ...(sessionId ? [["session", "当前会话"]] : []) ].map(([value, label]) => h("option", { value, key: value }, label))));
            const projectEditor = !sessionId && scope === "project" && h("div", { key: "project-directory", style: { display: "grid", gap: 8 } },
                row("项目目录", h("input", { "aria-label": "项目目录", value: cwdDraft, placeholder: "工作区绝对路径", onChange: e => setCwdDraft(e.target.value), onKeyDown: e => { if (e.key === "Enter" && !e.nativeEvent?.isComposing && e.keyCode !== 229) { e.preventDefault(); readProject(); } }, disabled: busy })),
                h("div", { className: "dshEnvActions" }, button("读取项目配置", readProject)),
                h("small", { className: "dshSubHint" }, projectChanged ? "目录尚未应用，请先读取项目配置。" : state ? `当前读取目录：${state.workspace}` : error ? "目录尚未读取成功，可修改目录后重试。" : "正在读取所选目录…"));
            if (!state) return h("section", { className: "dshSub dshEnvironment" }, h("h2", null, "工具与运行环境"), scopePicker, projectEditor, h("p", { role: error ? "alert" : "status" }, error || "正在读取环境…"), button("重新加载", load));
			return h("section", { className: "dshSub dshEnvironment" },
				h("h2", null, "工具与运行环境"),
				h("p", { className: "dshSubHint" }, `${state.os} / ${state.arch} · 本机 · ${permissionLabels[state.permissionMode] || state.permissionMode}`),
				h("p", null, `Shell：${state.effective.shellPath || "未找到"}；Python：${state.effective.pythonPath || "未找到"}`),
				state.effective.error && h("p", { role: "alert" }, state.effective.error),
				h("p", { className: "dshSubHint" }, "环境选择对后续执行生效；运行中的命令保留原环境，已有终端需重新创建。权限由会话权限设置独立管理。"),
				scopePicker,
				projectEditor,
				row("Shell", h("select", { value: preferences.shellKind || "", onChange: e => { field("shellKind", e.target.value); field("shellPath", ""); }, disabled: busy }, h("option", { value: "" }, "自动推荐"), state.shellCandidates.map(candidate => h("option", { value: candidate.kind, key: candidate.kind }, `${candidate.kind} · ${candidate.path}`)), preferences.shellKind && !state.shellCandidates.some(c => c.kind === preferences.shellKind) ? h("option", { value: preferences.shellKind }, `${preferences.shellKind}（当前不可用）`) : null)),
				row("指定 Shell 路径（可选）", h("input", { value: preferences.shellPath || "", onChange: e => field("shellPath", e.target.value), disabled: busy, placeholder: "绝对路径" })),
				row("Python", h("input", { value: preferences.pythonPath || "", onChange: e => field("pythonPath", e.target.value), disabled: busy, placeholder: "自动选择；或填写解释器绝对路径" })),
				row("信任并使用当前项目 .venv", h("input", { type: "checkbox", checked: !!preferences.useProjectPython, onChange: e => setPreferences(p => ({ ...p, useProjectPython: e.target.checked })), disabled: busy || !!preferences.pythonPath })),
				row("WPS 路径（可选）", h("input", { value: preferences.wpsPath || "", onChange: e => field("wpsPath", e.target.value), disabled: busy, placeholder: "自动定位；或填写 wps.exe 的绝对路径" })),
				h("details", null, h("summary", null, "工具链路径"), ["node", "rustc", "cargo", "git", "rg", "ffmpeg"].map(name => row(name, h("input", { key: name, value: preferences.toolchainPaths?.[name] || "", onChange: e => setPreferences(p => { const paths = { ...p.toolchainPaths }; if (e.target.value) paths[name] = e.target.value; else delete paths[name]; return { ...p, toolchainPaths: paths }; }), placeholder: "自动发现；或填写绝对路径", disabled: busy })))),
				h("div", { className: "dshEnvActions", "aria-label": "配置操作" }, button("保存", () => action({ action: "save", scope, preferences, expectedRevision: state.revision }, true), projectChanged),
				button("移除当前范围覆盖", () => action({ action: "reset", scope, expectedRevision: state.revision }, true), projectChanged), button("放弃修改并重新加载", () => { setResult(null); setCwdDraft(cwd); return load(); })),
                h("h3", null, "环境检查"),
                h("p", { className: "dshSubHint" }, "检查使用已保存的配置；修改后请先保存。"),
                h("div", { className: "dshEnvActions", "aria-label": "环境检查操作" },
				h("h3", null, "环境检查"),
				h("p", { className: "dshSubHint" }, "首次需要时自动检查并复用缓存。宿主可运行不代表当前工作区可运行。刷新只清理诊断记录。"),
				["shell", "python", "rustc", "cargo", "node", "wps"].map(name => button(`检查 ${name}`, () => action({ action: "probe", name, level: name === "wps" ? "locate" : "launch", refresh: true }), projectChanged)),
				button("检查宿主 Python", () => action({ action: "probeHost", name: "python" })),
				button("清空诊断缓存", () => action({ action: "clearCache" }))),
				result && h("p", { role: "status" }, `${result.id || ""} · ${result.executionWorld === "host" ? "宿主" : "所选执行环境"} · ${labels[result.status] || result.status}${result.error ? `：${result.error}` : ""}${result.output ? ` · ${result.output}` : ""}`),
				error && h("p", { role: "alert" }, error));
		}
		function RemoteExecutionSection({ api }) {
			const h=react.createElement;
			const [rows,setRows]=react.useState([]),[busy,setBusy]=react.useState(false),[error,setError]=react.useState("");
			const [form,setForm]=react.useState({host:"",user:"",port:22,workspace:"",helper:"dsh-remote-helper",configFile:""});
			const request=async data=>{const response=await fetch("/__dsh-remote-execution",{method:"POST",headers:{"Content-Type":"application/json"},credentials:"same-origin",body:JSON.stringify(data)});const value=await response.json();if(!response.ok||value.error)throw new Error(value.error||"远端操作失败");return value;};
			const load=async()=>{try{const result=await request({action:"list"});setRows(result.items||[]);}catch(e){setError(messageOf(e));}};
			react.useEffect(()=>{load();},[]);
			const connect=async()=>{setBusy(true);setError("");try{const connection={...form,id:crypto.randomUUID(),port:Number(form.port)};const verified=await request({action:"connect",connection});const reply=await api.workspace.create({kind:"ssh-execution",source:verified.connection.id,path:verified.handshake.workspace});if(!reply.result.ok)throw new Error(reply.result.error.message);await load();}catch(e){setError(messageOf(e));}finally{setBusy(false);}};
			return h("section",{className:"dshSub dshRemoteExecution"},h("h2",null,"远端执行"),h("p",null,"当前 Agent 在远端工作区执行文件和命令；模型连接保留在本机。远端独立 Harness 的 SSH 隧道入口继续独立使用。"),h("p",{className:"dshSubHint"},"连接前需在远端安装同版本 dsh-remote-helper，并显式授权工作目录；使用已验证的 known_hosts 和现有 SSH 身份，不复制模型凭据或自动安装服务。"),
				[["host","SSH 主机或配置别名"],["user","用户名（可选）"],["port","SSH 端口"],["workspace","远端工作目录"],["helper","远端 helper 路径"],["configFile","本机 SSH 配置文件（可选）"]].map(([name,label])=>h("label",{key:name,style:{display:"grid",gap:6,margin:0}},label,h("input",{disabled:busy,value:form[name],type:name==="port"?"number":"text",onChange:e=>setForm(p=>({...p,[name]:e.target.value}))}))),
				h("button",{type:"button",disabled:busy,onClick:connect},busy?"正在核验远端…":"核验并添加工作区"),error&&h("p",{role:"alert"},error),
				rows.map(row=>h("div",{key:row.connection.id,style:{marginTop:16}},h("strong",null,`${row.connection.user?row.connection.user+"@":""}${row.connection.host} · ${row.handshake.os}/${row.handshake.arch}`),h("p",null,row.handshake.workspace),h("p",{className:"dshSubHint"},`协议 ${row.handshake.protocolVersion} · ${row.handshake.permissionCeiling} · 最近核验身份 ${row.handshake.contextId.slice(0,12)}`),h("button",{disabled:busy,onClick:async()=>{setBusy(true);try{await request({action:"connect",connection:row.connection});await load();}catch(e){setError(messageOf(e));}finally{setBusy(false);}}},"重新核验"),h("button",{disabled:busy,onClick:async()=>{setBusy(true);try{await request({action:"remove",id:row.connection.id});await load();}catch(e){setError(messageOf(e));}finally{setBusy(false);}}},"移除连接"))));
		}
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
				(0, react_jsx_runtime.jsx)("div", { className: "dshSubHint", children: "配置子智能体的默认路由、模型和运行限制。模型留空时继承父会话，其余字段含义见各项说明。" }),
				(0, react_jsx_runtime.jsx)("div", { className: "dshSubSectionTitle", children: (0, react_jsx_runtime.jsx)("h3", { children: "默认值与限制" }) }),
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
			"unsaved.title": "未保存的修改",
			"unsaved.description": "当前页面有未保存的修改，关闭后这些修改会丢失。",
			"unsaved.continue": "继续编辑",
			"unsaved.discard": "放弃修改并关闭",
			"openDocument": "打开配置文件",
			"openDocument.error": "无法打开配置文件",
			"general.nav": "通用设置",
			"directoryPicker.title": "目录选择器",
			"directoryPicker.label": "选择方式",
			"directoryPicker.hint": "以后选择工作区、垃圾槽等目录时使用此方式。",
			"directoryPicker.browse": "网页目录浏览",
			"directoryPicker.native": "系统资源管理器",
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
			"unsaved.title": "Unsaved changes",
			"unsaved.description": "This page has unsaved changes. Closing it will discard them.",
			"unsaved.continue": "Continue editing",
			"unsaved.discard": "Discard and close",
			"openDocument": "Open configuration file",
			"openDocument.error": "Could not open configuration file",
			"general.nav": "General",
			"directoryPicker.title": "Directory picker",
			"directoryPicker.label": "Selection method",
			"directoryPicker.hint": "Use this method for workspace, scratch and other directory selections.",
			"directoryPicker.browse": "Web directory browser",
			"directoryPicker.native": "System File Explorer",
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
			if(typeof document!=="undefined"&&!document.querySelector("style[data-dsh-unsaved-dialog]")){const style=document.createElement("style");style.dataset.dshUnsavedDialog="";style.textContent=".dshUnsavedDialog{position:absolute;z-index:5;right:24px;bottom:24px;width:min(380px,calc(100% - 48px));box-sizing:border-box;padding:16px;border:1px solid var(--dsw-alias-border-l2);border-radius:14px;background:var(--dsw-alias-bg-layer-1);box-shadow:var(--dsw-shadow-lv3);color:var(--dsw-alias-label-primary)}.dshUnsavedDialog strong{display:block;font-size:14px}.dshUnsavedDialog p{margin:6px 0 14px;color:var(--dsw-alias-label-secondary);font-size:13px;line-height:20px}.dshUnsavedActions{display:flex;justify-content:flex-end;gap:8px;flex-wrap:wrap}.dshUnsavedActions button{min-height:34px;padding:6px 12px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:transparent;color:inherit;font:inherit;cursor:pointer}.dshUnsavedActions button:hover{background:var(--dsw-alias-interactive-bg-hover)}.dshUnsavedActions .dshUnsavedDiscard{color:var(--dsw-alias-state-error-primary);border-color:var(--dsw-alias-state-error-primary)}";document.head.appendChild(style)}
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
				inject: () => ({ t }),
				children: { "settings.general.item": {
					kind: "list",
					scope: "root"
				} }
			}, GeneralSection));
            ctx.slots.inject("conversation.context.experience",()=>ctx.slots.register({name:"conversation.context.experience",inject:sessionId=>({api:connection.api,sessionId})},ExperiencePanel));
			ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "environment", order: 24, label: "工具与运行环境" }, EnvironmentSection));
			ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "remote-execution", order: 24.5, label: "远端执行", inject:()=>({api:connection.api}) }, RemoteExecutionSection));
			ctx.slots.inject("conversation.view", () => ctx.slots.register({ name: "conversation.view", id: "environment", order: 22, label: "运行环境", inject: sessionId => ({ sessionId }) }, EnvironmentSection));
			ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "memory",
				order: 20,
				label: "记忆与上下文",inject:()=>({api:connection.api}),children:{"settings.memory.import":{kind:"single",scope:"root"}}
			}, MemorySection));

			ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "security",
				order: 25,
				label: "安全盾"
			}, () => (0, react_jsx_runtime.jsx)(SecuritySection, { api: connection.api })));
		}
		//#endregion
		exports.SubagentDefaultsSection = SubagentSection;
		exports.SettingsDocumentStore = SettingsDocumentStore;
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map
