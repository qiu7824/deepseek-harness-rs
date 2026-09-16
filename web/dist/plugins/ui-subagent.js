window.__ModuleLoader__.load({
	id: "@deepseek-ai/dsh-client-ui-subagent",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		let react_jsx_runtime = require("react/jsx-runtime");
		let react = require("react");
		let _deepseek_ai_dsh_client_runtime_client = require("@deepseek-ai/dsh-client-runtime/client");
		let _deepseek_ai_dsh_client_ui_primitives = require("@deepseek-ai/dsh-client-ui-primitives");
        const SubagentDefaultsSection = require("@deepseek-ai/dsh-client-ui-settings-general").SubagentDefaultsSection;
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-subagent\src\client\SubagentCatalogAction.module.css.mjs
		const css$1 = ".dshCollaborationPanel{display:grid;gap:16px;max-height:70vh;overflow:auto;overflow-wrap:anywhere}.dshTeamToolbar{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshTeamToolbar label{display:flex;gap:8px;align-items:center}.dshTeamTabs{display:flex;gap:4px;border-bottom:1px solid var(--dsw-alias-border-l2);padding-bottom:8px}.dshTeamTabs button{font:inherit;border:0;border-radius:8px;padding:7px 12px;cursor:pointer;color:var(--dsw-alias-label-secondary);background:transparent}.dshTeamTabs button[aria-selected=true]{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-primary)}.dshTeamCard{display:grid;gap:10px;border:1px solid var(--dsw-alias-border-l2);border-radius:12px;padding:14px;margin:10px 0;min-width:0}.dshTeamCard fieldset{border:0;padding:0;margin:0;display:grid;gap:10px;min-width:0}.dshTeamCard legend{font-weight:600;margin-bottom:10px}.dshTeamField{display:grid;gap:6px;font-size:13px}.dshTeamField input,.dshTeamField textarea,.dshTeamField select,.dshTeamToolbar select,.dshTeamMessage input{box-sizing:border-box;min-width:0;width:100%;min-height:36px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-specific-input-major);color:var(--dsw-alias-label-primary);font:inherit;padding:7px 10px}.dshTeamField textarea{resize:vertical;line-height:1.6}.dshTeamToolbar select{width:auto}.dshTeamField select[multiple]{min-height:70px}.dshTeamButton:disabled{opacity:.45;cursor:default}.dshTeamError{color:var(--dsw-alias-state-error-primary);font-size:13px}.dshTeamMessage{display:flex;gap:8px}.dshTeamMessage input{flex:1}.dshTeamRole{border-top:1px solid var(--dsw-alias-border-l2);padding-top:10px}.dshTeamRole summary{cursor:pointer;padding:6px 0}.dshTeamRole[open] .dshTeamField{margin:10px 0}.dshTeamCard p{white-space:pre-wrap}[data-agent-team-board] h3{margin:0;font-size:14px;line-height:22px;font-weight:600}[data-agent-team-board] p{margin:0}.dshTeamTrigger{display:inline-flex;align-items:center;gap:5px;min-height:30px;padding:4px 8px;border:0;border-radius:8px;background:transparent;color:var(--dsw-alias-label-secondary);font:inherit;font-size:12px;cursor:pointer}.dshTeamTrigger:hover,.dshTeamButton:hover{background:var(--dsw-alias-interactive-bg-hover)}.dshTeamSettings{display:grid;gap:16px}.dshTeamHint{margin:0;color:var(--dsw-alias-label-tertiary);font-size:13px;line-height:1.7}.dshTeamSetting{display:flex;align-items:center;justify-content:space-between;gap:16px;font-size:14px}.dshTeamSetting select,.dshTeamButton{border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:6px 12px;background:var(--dsw-specific-menu);color:var(--dsw-alias-label-primary);font:inherit}.dshTeamButton{cursor:pointer}.dshTeamSetting input{appearance:none;-webkit-appearance:none;position:relative;flex:none;width:36px;height:22px;margin:0;border:0;border-radius:20px;background:var(--dsw-alias-border-l3,#c9cdd4);cursor:pointer;transition:background .15s}.dshTeamSetting input:before{content:\"\";position:absolute;left:3px;top:3px;width:16px;height:16px;border-radius:50%;background:#fff;box-shadow:0 1px 3px #0002;transition:transform .15s}.dshTeamSetting input:checked{background:var(--dsw-alias-state-business-primary,#4d6bfe)}.dshTeamSetting input:checked:before{transform:translateX(14px)}.dshTeamSetting input:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary,#4d6bfe);outline-offset:3px}.dshTeamSetting input:disabled{opacity:.45;cursor:default}.dshTeamTrigger:focus-visible,.dshTeamButton:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.HfG9eW_root{position:relative}.HfG9eW_trigger{min-height:28px;color:var(--dsw-alias-label-tertiary);cursor:pointer;background:0 0;border:0;border-radius:6px;align-items:center;gap:3px;padding:3px 2px;font-size:12px;line-height:18px;display:inline-flex}.HfG9eW_count{margin:0 5px}.HfG9eW_activitySlot{flex:none;width:10px;height:10px;display:inline-flex}.HfG9eW_trigger:hover,.HfG9eW_trigger:focus-visible{color:var(--dsw-alias-label-secondary)}.HfG9eW_trigger svg{transition:transform .12s}.HfG9eW_triggerOpen{transform:rotate(180deg)}.HfG9eW_menu{z-index:100;box-sizing:border-box;background:var(--dsw-specific-menu);--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2);--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2);width:336px;max-width:min(400px,100vw - 32px);max-height:min(560px,100vh - 140px);box-shadow:var(--dsw-shadow-lv3);border-radius:12px;flex-direction:column;padding:4px;display:flex;position:absolute;top:calc(100% + 5px);left:0;overflow:auto}.HfG9eW_node{min-width:0;position:relative}.HfG9eW_menu>.HfG9eW_node{margin-left:-3px}.HfG9eW_row{box-sizing:border-box;width:100%;min-height:50px;color:var(--dsw-alias-label-primary);text-align:left;cursor:pointer;background:0 0;border:0;border-radius:8px;outline:none;align-items:flex-start;gap:8px;padding:7px 8px 7px 11px;font-size:13px;line-height:18px;display:flex;position:relative}.HfG9eW_row:hover>.HfG9eW_clickarea,.HfG9eW_row:focus-visible>.HfG9eW_clickarea{background:var(--dsw-alias-interactive-bg-hover)}.HfG9eW_clickarea{box-sizing:border-box;border-radius:8px;flex:1;align-self:stretch;align-items:flex-start;gap:8px;min-width:0;margin:-7px -8px;padding:7px 8px;display:flex}.HfG9eW_row>[data-state],.HfG9eW_clickarea>[data-state]{margin-top:4px}.HfG9eW_disabled{color:var(--dsw-alias-label-dimmed);cursor:not-allowed}.HfG9eW_disabled:hover{background:0 0}.HfG9eW_loadingRow{cursor:default}.HfG9eW_disclosure,.HfG9eW_disclosureSpace{flex:none;width:14px;height:18px}.HfG9eW_disclosure{color:var(--dsw-alias-label-tertiary);cursor:pointer;background:0 0;border:0;justify-content:center;align-items:center;padding:0;transition:transform .12s;display:inline-flex}.HfG9eW_disclosure:hover{color:var(--dsw-alias-label-primary)}.HfG9eW_disclosureOpen{transform:rotate(90deg)}.HfG9eW_content{flex-direction:column;flex:1;min-width:0;display:flex}.HfG9eW_label,.HfG9eW_summary{text-overflow:ellipsis;white-space:nowrap;overflow:hidden}.HfG9eW_label{color:inherit;font-weight:400}.HfG9eW_summary,.HfG9eW_metrics{color:var(--dsw-alias-label-tertiary);font-size:11px;line-height:16px}.HfG9eW_metrics{font-variant-numeric:tabular-nums;text-align:right;white-space:nowrap;flex:none;grid-template-rows:18px 16px;display:grid}.HfG9eW_metricToken{grid-row:1;line-height:18px}.HfG9eW_metricDuration{grid-row:2}.HfG9eW_children{margin-left:18px;padding-left:4px;position:relative}.HfG9eW_children:before,.HfG9eW_children>.HfG9eW_node:before{content:\"\";border-left:1px solid var(--dsw-alias-border-l2);position:absolute;left:0}.HfG9eW_children:before{height:26px;top:-26px}.HfG9eW_children[aria-busy=true]:before{content:none}.HfG9eW_children>.HfG9eW_node:before{top:0;bottom:0;left:-4px}.HfG9eW_children>.HfG9eW_node:last-child:before{height:17px;bottom:auto}.HfG9eW_children>.HfG9eW_node>.HfG9eW_row:before{content:\"\";border-top:1px solid var(--dsw-alias-border-l2);width:14px;position:absolute;top:16px;left:-4px}.HfG9eW_notice,.HfG9eW_error{color:var(--dsw-alias-label-tertiary);padding:10px 12px;font-size:12px;line-height:18px}.HfG9eW_error{color:var(--dsw-alias-state-error-primary);justify-content:space-between;align-items:center;gap:12px;display:flex}.HfG9eW_refresh{color:inherit;cursor:pointer;background:0 0;border:0;border-radius:6px;flex:none;align-items:center;gap:4px;padding:4px 6px;display:inline-flex}.HfG9eW_refresh:hover{background:var(--dsw-alias-interactive-bg-hover)}";
		const tagId$1 = "@deepseek-ai/dsh-client-ui-subagent/SubagentCatalogAction.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId$1) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-subagent";
			tag.dataset.pluginCss = tagId$1;
			tag.textContent = css$1 + '.dshTeamModal{width:min(760px,calc(100vw - 32px));max-width:calc(100vw - 32px)}.dshTeamModalContent{box-sizing:border-box;width:100%;min-width:0}.dshTeamCreate>summary{cursor:pointer;list-style:none;display:inline-flex;align-items:center;min-height:32px}.dshTeamCreate>summary:before{content:"+";margin-right:8px}.dshTeamCreate[open]>summary:before{content:"−"}.dshTeamCreate>summary:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:3px}';
			document.head.appendChild(tag);
		}
		var SubagentCatalogAction_module_css_default = {
			"triggerOpen": "HfG9eW_triggerOpen",
			"disclosureSpace": "HfG9eW_disclosureSpace",
			"notice": "HfG9eW_notice",
			"metricToken": "HfG9eW_metricToken",
			"refresh": "HfG9eW_refresh",
			"clickarea": "HfG9eW_clickarea",
			"disabled": "HfG9eW_disabled",
			"row": "HfG9eW_row",
			"content": "HfG9eW_content",
			"trigger": "HfG9eW_trigger",
			"summary": "HfG9eW_summary",
			"metrics": "HfG9eW_metrics",
			"loadingRow": "HfG9eW_loadingRow",
			"label": "HfG9eW_label",
			"count": "HfG9eW_count",
			"metricDuration": "HfG9eW_metricDuration",
			"disclosure": "HfG9eW_disclosure",
			"children": "HfG9eW_children",
			"error": "HfG9eW_error",
			"root": "HfG9eW_root",
			"disclosureOpen": "HfG9eW_disclosureOpen",
			"node": "HfG9eW_node",
			"menu": "HfG9eW_menu",
			"activitySlot": "HfG9eW_activitySlot"
		};
		//#endregion
		//#region lib/types/client/SubagentCatalogAction.js
		function diagnosticReason(entry, t) {
			switch (entry.reason) {
				case "corrupt": return t("diagnostic.corrupt");
				case "unsupported": return t("diagnostic.unsupported");
				case "unavailable": return t("diagnostic.unavailable");
			}
		}
		function treeItems(root) {
			return root === null ? [] : Array.from(root.querySelectorAll("[role=\"treeitem\"]:not([aria-disabled=\"true\"])"));
		}
		/** Compact token count shared in shape with the conversation stats strip. */
		function formatTokens(value) {
			const scaled = (next) => next >= 100 ? String(Math.round(next)) : String(Math.round(next * 10) / 10);
			if (value < 1e3) return String(value);
			if (value < 1e6) return `${scaled(value / 1e3)}K`;
			return `${scaled(value / 1e6)}M`;
		}
		/** Sum the four disjoint durable provider-usage buckets. */
		function tokenTotal(usage) {
			return usage === void 0 ? void 0 : usage.uncachedInputTokens + usage.outputTokens + usage.cacheReadTokens + usage.cacheWriteTokens;
		}
		/** Exact whole-second active-turn duration for one catalog row. */
		function activityDuration(summary, activity, now) {
			if (summary === void 0) return void 0;
			const timing = summary.projectionValues?.subagentTiming;
			if (timing === void 0) return void 0;
			if (timing.active === void 0) return timing.settledMs;
			const end = activity === "running" ? now : timing.active.through;
			return timing.settledMs + Math.max(0, end - timing.active.since);
		}
		function splitDuration(ms) {
			const totalSeconds = Math.floor(Math.max(0, ms) / 1e3);
			const totalMinutes = Math.floor(totalSeconds / 60);
			const totalHours = Math.floor(totalMinutes / 60);
			return {
				seconds: totalSeconds % 60,
				minutes: totalMinutes % 60,
				hours: totalHours % 24,
				days: Math.floor(totalHours / 24),
				totalMinutes,
				totalHours
			};
		}
		/** Format a duration with decreasing visual precision at larger scales. */
		function formatDuration(ms, t) {
			const { seconds, minutes, hours, days, totalMinutes, totalHours } = splitDuration(ms);
			if (days >= 365) {
				const years = Math.floor(days / 365);
				const months = Math.floor(days % 365 / 30);
				return months === 0 ? t("duration.years", { years }) : t("duration.yearsMonths", {
					years,
					months
				});
			}
			if (days >= 30) {
				const months = Math.floor(days / 30);
				const remainingDays = days % 30;
				return remainingDays === 0 ? t("duration.months", { months }) : t("duration.monthsDays", {
					months,
					days: remainingDays
				});
			}
			if (days > 0) return hours === 0 ? t("duration.days", { days }) : t("duration.daysHours", {
				days,
				hours
			});
			if (totalHours > 0) return t("duration.hours", {
				hours: totalHours,
				minutes: String(minutes).padStart(2, "0"),
				seconds: String(seconds).padStart(2, "0")
			});
			if (totalMinutes > 0) return t("duration.minutes", {
				minutes: totalMinutes,
				seconds: String(seconds).padStart(2, "0")
			});
			return t("duration.seconds", { seconds });
		}
		/** Preserve exact whole seconds for hover and accessible naming. */
		function formatExactDuration(ms, t) {
			const { seconds, minutes, hours, days } = splitDuration(ms);
			return days === 0 ? formatDuration(ms, t) : t("duration.exactDays", {
				days,
				hours: String(hours).padStart(2, "0"),
				minutes: String(minutes).padStart(2, "0"),
				seconds: String(seconds).padStart(2, "0")
			});
		}
		const NO_DESCENDANTS = {
			count: 0,
			runningCount: 0
		};
		/** Read only public assistant text and terminal state from the child's own history. */
		function childProgress(history, activity) {
			let state = "inactive", preview = "", stream = "";
			for (const item of history?.events ?? []) {
				const event = item.event ?? item, data = event.data ?? {};
				if (event.type === "subagent/descriptor") { preview = ""; stream = ""; state = "inactive"; }
				if (event.type === "turn/start") state = "running";
				if (event.type === "step/start") stream = "";
				if (event.type === "assistant/chunk" && data.chunk?.type === "text-delta") {
					stream += data.chunk.text ?? "";
					preview = stream;
				}
				if (event.type === "chunkrow/text-chunks") {
					for (const chunk of data.chunks ?? []) stream += typeof chunk === "string" ? chunk : chunk.text ?? "";
					if (stream) preview = stream;
				}
				if (event.type === "assistant/message") {
					const text = (data.message?.content ?? data.content ?? []).filter((block) => block.type === "text").map((block) => block.text).join("\n");
					if (text) preview = text;
				}
				if (event.type === "turn/end") {
					const reason = data.reason?.kind;
					state = reason === "completed" ? "completed" : reason === "error" ? "failed" : reason === "blocked" ? "blocked" : ["aborted", "interrupted", "max-tokens"].includes(reason) ? "stopped" : "inactive";
				}
			}
			return { state: activity === "running" ? "running" : state === "running" ? "inactive" : state, preview: preview.replace(/\s+/g, " ").trim().slice(0, 280) };
		}
		function progressDot(state) {
			return state === "running" ? "ongoing" : state === "failed" ? "error" : ["stopped", "blocked", "unavailable"].includes(state) ? "warning" : state === "completed" ? "done" : void 0;
		}
		function useChildProgress(address, activity, loadProgress) {
			const [value, setValue] = (0, react.useState)({ state: activity === "running" ? "running" : "inactive", preview: "" });
			(0, react.useEffect)(() => {
				if (!address) return;
				let cancelled = false, timer;
				const read = async () => {
					try {
						const history = await loadProgress(address);
						if (!cancelled) setValue(childProgress(history, activity));
					} catch {
						if (!cancelled) setValue({ state: activity === "running" ? "running" : "unavailable", preview: "" });
					} finally {
						if (!cancelled && activity === "running") timer = setTimeout(read, 1500);
					}
				};
				read();
				return () => { cancelled = true; clearTimeout(timer); };
			}, [address?.parentSessionId, address?.childSessionId, address?.mode, activity, loadProgress]);
			return activity === "running" && value.state !== "running" ? { ...value, state: "running" } : value;
		}
		function ChildProgressContent({ parentSessionId, entry, label, mode, loadProgress, t }) {
			const progress = useChildProgress({ parentSessionId, childSessionId: entry.id, mode: entry.mode }, entry.activity, loadProgress);
			return (0, react_jsx_runtime.jsx)(ProgressContent, { progress, label, mode, t });
		}
		function ProgressContent({ progress, label, mode, t }) {
			return (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
				(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, { state: progressDot(progress.state) }),
				(0, react_jsx_runtime.jsxs)("span", { className: SubagentCatalogAction_module_css_default.content, "data-subagent-state": progress.state, children: [
					(0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.label, children: label }),
					(0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.summary, children: `${mode} · ${t("progress." + progress.state)}` }),
					progress.preview && (0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.summary, title: progress.preview, children: progress.preview })
				] })
			] });
		}
		/** A returned continuation id is an exact link; task descriptions never establish identity. */
		function subagentToolModel(block) {
			const settled = "kind" in block;
			let args = {};
			try { args = JSON.parse((settled ? block.call?.argsRaw : block.argsRaw) ?? "{}"); } catch {}
			const output = settled ? (block.content ?? []).filter((item) => item.type === "text").map((item) => item.text).join("\n") : "";
			const childId = /^started continuable subagent ([a-zA-Z0-9-]+)$/.exec(output.trim())?.[1];
			return { label: typeof args.description === "string" ? args.description : "", prompt: typeof args.prompt === "string" ? args.prompt : "", output, childId, state: !settled ? "running" : block.error?.code === "interrupted" ? "stopped" : block.isError ? "failed" : childId ? "inactive" : "completed" };
		}
        function subagentFileLinks(text, prefix) {
            const files = new Map();
            // Preserve fenced/indented code and inline code verbatim. Only explicit
            // Markdown links with local destinations gain the existing file opener.
            const protectedRanges = [];
            const code = /(^|\n)( {0,3})(`{3,}|~{3,})[^\n]*\n[\s\S]*?(?:\n\2\3[^\n]*(?=\n|$)|$)|(^|\n)(?: {4}|\t)[^\n]*(?:\n(?: {4}|\t)[^\n]*)*|(`+)([^`]|(?!\5)`)*?\5/g;
            for (const match of text.matchAll(code)) protectedRanges.push([match.index, match.index + match[0].length]);
            const rewritten = text.replace(/\[([^\[\]\n]+)\]\((?:<([^>\n]+)>|([^\s()]+))\)/g, (whole, label, bracketed, plain, offset) => {
                if (files.size >= 256 || protectedRanges.some(([start, end]) => offset >= start && offset < end)) return whole;
                let escapes = 0; for (let i = offset - 1; i >= 0 && text[i] === "\\"; i--) escapes++;
                if (escapes % 2 || text[offset - 1] === "!") return whole;
                const target = bracketed ?? plain;
                if (/[\u0000-\u001f\u007f]/.test(target) || target.startsWith("//")) return whole;
                const drive = /^[A-Za-z]:[\\/]/.test(target);
                if (!drive && /^[A-Za-z][A-Za-z0-9+.-]*:/.test(target)) return whole;
                if (!drive && !(target.startsWith("/") || target.startsWith("./") || target.startsWith("../") || /^[^?#]+\.[A-Za-z0-9]{1,16}(?:#[^\s]*)?$/.test(target))) return whole;
                const href = prefix + files.size;
                files.set(href, target);
                return `[${label}](${href})`;
            });
            return { text: rewritten, files };
        }
        function SubagentMarkdownOutput({ text, openFile, t }) {
            const id = react.useId();
            const prefix = new URL("/__dsh-file-link/" + encodeURIComponent(id) + "/", document.baseURI).href;
            const value = react.useMemo(() => typeof openFile === "function" ? subagentFileLinks(text, prefix) : { text, files: new Map() }, [text, prefix, openFile]);
            const open = event => {
                if (event.button > 1) return;
                const anchor = event.target.closest?.("a[href]");
                if (!anchor || !event.currentTarget.contains(anchor)) return;
                let key; try { key = new URL(anchor.href).href; } catch { return; }
                const target = value.files.get(key);
                if (target === undefined) return;
                event.preventDefault(); event.stopPropagation(); openFile(target);
            };
            return (0, react_jsx_runtime.jsx)("div", { className: "dsh-subagent-markdown", onClick: open, onAuxClick: open, children: (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.MarkdownText, { text: value.text, codeLabels: { copyLabel: t("copy"), copiedLabel: t("copied") } }) });
        }
		function SubagentToolRow({ block, parentSessionId, sessionsStore, openChild, refresh, loadProgress, inspect, openFile, t }) {
			const model = subagentToolModel(block);
			const snapshot = (0, react.useSyncExternalStore)(sessionsStore.subscribe, sessionsStore.getSnapshot, sessionsStore.getSnapshot);
			const entry = snapshot.subagentsByParent[parentSessionId]?.entries.find((item) => item.kind === "child" && item.id === model.childId);
			const [open, setOpen] = (0, react.useState)(false);
			(0, react.useEffect)(() => { if (model.childId) refresh(parentSessionId); }, [model.childId, parentSessionId, refresh]);
			const address = model.childId ? { parentSessionId, childSessionId: model.childId, mode: "continuable" } : null;
			const progress = useChildProgress(address, entry?.activity ?? (snapshot.byId[model.childId]?.running ? "running" : "inactive"), loadProgress);
			const state = address ? progress.state : model.state;
			return (0, react_jsx_runtime.jsxs)("div", { className: "dsh-subagent-tool", "data-tool": "subagent", "data-state": state, children: [
				(0, react_jsx_runtime.jsxs)("button", { type: "button", className: "dsh-subagent-tool-trigger", title: `${t("tool.title")} · ${t("progress." + state)}`, "aria-expanded": open, onClick: () => setOpen(!open), children: [
					(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, { state: progressDot(state) }),
					(0, react_jsx_runtime.jsx)("span", { className: "dshReplyHintIcon", "aria-hidden": true, children: (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconAgentPresetOutline16, { size: 14 }) }),
					(0, react_jsx_runtime.jsx)("span", { className: "dshReplyHintLabel", children: t("tool.title") }),
					(0, react_jsx_runtime.jsx)("span", { className: "dsh-subagent-tool-label", children: model.label }),
					(0, react_jsx_runtime.jsx)("span", { className: "dshReplyHintLabel", children: t("progress." + state) }),
					(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconChevronDownOutline14, {})
				] }),
				open && (0, react_jsx_runtime.jsxs)("div", { className: "dsh-subagent-tool-body", children: [
					address ? (0, react_jsx_runtime.jsx)("div", { className: "dsh-subagent-tool-progress", children: (0, react_jsx_runtime.jsx)(ProgressContent, { progress, label: model.label || model.childId, mode: t("mode.continuable"), t }) }) : (0, react_jsx_runtime.jsx)("p", { children: t("progress." + model.state) }),
					model.prompt && (0, react_jsx_runtime.jsx)("p", { children: model.prompt }),
					!address && model.output && (0, react_jsx_runtime.jsx)(SubagentMarkdownOutput, { text: model.output, openFile, t }),
					address && (0, react_jsx_runtime.jsx)("button", { type: "button", onClick: () => openChild(address), children: t("tool.open") }),
					inspect && (0, react_jsx_runtime.jsx)("button", { type: "button", onClick: inspect, children: t("tool.details") })
				] })
			] });
		}
		if (typeof document !== "undefined" && !document.querySelector("style[data-subagent-progress]")) {
			const style = document.createElement("style");
			style.dataset.subagentProgress = "true";
			style.textContent = ".dsh-subagent-tool{margin:4px 0;color:var(--dsw-alias-label-secondary);font-size:13px}.dsh-subagent-tool-trigger{display:flex;align-items:center;gap:8px;width:100%;min-height:32px;border:0;background:none;color:inherit;text-align:left;cursor:pointer}.dsh-subagent-tool-label{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dsh-subagent-tool-body{margin:4px 0 8px 18px;padding:8px 12px;border-left:1px solid var(--dsw-alias-border-l2);overflow-wrap:anywhere}.dsh-subagent-tool-body p{margin:6px 0}.dsh-subagent-tool-body pre{white-space:pre-wrap;max-height:220px;overflow:auto}.dsh-subagent-tool-body>button{background:none;color:var(--dsw-alias-label-link);border:0;padding:6px 12px 6px 0;cursor:pointer}.dsh-subagent-tool-progress{display:flex;gap:8px;align-items:flex-start}";
			style.textContent += "@media(max-width:520px){.HfG9eW_menu{position:fixed;left:16px;right:16px;top:var(--dsh-subagent-menu-top,96px);width:auto;max-width:none;max-height:calc(100dvh - var(--dsh-subagent-menu-top,96px) - 16px)}}";
			document.head.appendChild(style);
		}
		/** Render the known direct-child shape while its authoritative catalog hydrates. */
		function CatalogLoadingRows({ parentSessionId, summaries, level, t }) {
			const children = Object.values(summaries).filter((summary) => summary.origin === "subagent" && summary.parentId === parentSessionId);
			if (children.length === 0) return (0, react_jsx_runtime.jsx)("div", {
				className: SubagentCatalogAction_module_css_default.notice,
				children: t("loading.label")
			});
			return children.map((summary) => (0, react_jsx_runtime.jsx)("div", {
				className: SubagentCatalogAction_module_css_default.node,
				children: (0, react_jsx_runtime.jsxs)("div", {
					role: "treeitem",
					"aria-disabled": "true",
					"aria-level": level,
					"aria-label": t("loading.aria"),
					className: `${SubagentCatalogAction_module_css_default.row} ${SubagentCatalogAction_module_css_default.disabled} ${SubagentCatalogAction_module_css_default.loadingRow}`,
					children: [
						(0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.disclosureSpace }),
						(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, { state: summary.running ? "ongoing" : void 0 }),
						(0, react_jsx_runtime.jsx)("span", {
							className: SubagentCatalogAction_module_css_default.content,
							children: (0, react_jsx_runtime.jsx)("span", {
								className: SubagentCatalogAction_module_css_default.label,
								children: t("loading.label")
							})
						})
					]
				})
			}, summary.id));
		}
		/** Render one catalog level and recurse only through explicitly expanded rows. */
		function CatalogRows({ parentSessionId, catalog, catalogs, summaries, expanded, level, now, openChild, refresh, loadProgress, toggleBranch, closeCatalog, t }) {
			const emptyLoading = catalog.state === "loading" && catalog.entries.length === 0;
			const reserveDisclosure = catalog.entries.some((entry) => entry.kind === "child" && entry.hasChildren);
			return (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
				emptyLoading && (0, react_jsx_runtime.jsx)(CatalogLoadingRows, {
					parentSessionId,
					summaries,
					level,
					t
				}),
				catalog.state === "error" && (0, react_jsx_runtime.jsxs)("div", {
					className: SubagentCatalogAction_module_css_default.error,
					children: [(0, react_jsx_runtime.jsx)("span", { children: catalog.error?.message ?? t("load.error") }), (0, react_jsx_runtime.jsxs)("button", {
						type: "button",
						className: SubagentCatalogAction_module_css_default.refresh,
						onClick: () => {
							refresh(parentSessionId);
						},
						children: [(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconRefreshOutline14, {}), t("retry")]
					})]
				}),
				catalog.entries.map((entry) => {
					if (entry.kind === "diagnostic") {
						const reason = diagnosticReason(entry, t);
						return (0, react_jsx_runtime.jsx)("div", {
							className: SubagentCatalogAction_module_css_default.node,
							children: (0, react_jsx_runtime.jsxs)("div", {
								role: "treeitem",
								"aria-disabled": "true",
								"aria-level": level,
								"aria-label": `${entry.id} ${reason}`,
								className: `${SubagentCatalogAction_module_css_default.row} ${SubagentCatalogAction_module_css_default.disabled}`,
								title: reason,
								children: [
									reserveDisclosure && (0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.disclosureSpace }),
									(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, { state: "error" }),
									(0, react_jsx_runtime.jsxs)("span", {
										className: SubagentCatalogAction_module_css_default.content,
										children: [(0, react_jsx_runtime.jsx)("span", {
											className: SubagentCatalogAction_module_css_default.label,
											children: entry.id
										}), (0, react_jsx_runtime.jsx)("span", {
											className: SubagentCatalogAction_module_css_default.summary,
											children: reason
										})]
									})
								]
							})
						}, entry.id);
					}
					const childCatalog = catalogs[entry.id];
					const isExpanded = expanded.has(entry.id);
					const knownLeaf = !entry.hasChildren;
					const childLoading = childCatalog === void 0 || childCatalog.state === "loading" && childCatalog.entries.length === 0;
					const summary = summaries[entry.id];
					const label = entry.label ?? entry.id;
					const mode = entry.mode === "one-shot" ? t("mode.oneShot") : t("mode.continuable");
					const totalTokens = tokenTotal(summary?.projectionValues?.tokenUsage);
					const durationMs = activityDuration(summary, entry.activity, now);
					const tokenMetric = totalTokens === void 0 ? void 0 : `${formatTokens(totalTokens)} tok`;
					const durationMetric = durationMs === void 0 ? void 0 : {
						compact: formatDuration(durationMs, t),
						exact: formatExactDuration(durationMs, t)
					};
					const metrics = [tokenMetric, durationMetric?.exact].filter((value) => value !== void 0).join(" · ");
					const open = () => {
						openChild({
							parentSessionId,
							childSessionId: entry.id,
							mode: entry.mode
						});
						closeCatalog();
					};
					const handleKey = (event) => {
						if (event.key === "Enter" || event.key === " ") {
							event.preventDefault();
							event.stopPropagation();
							open();
						} else if (event.key === "ArrowRight" && !knownLeaf && !isExpanded || event.key === "ArrowLeft" && isExpanded) {
							event.preventDefault();
							event.stopPropagation();
							toggleBranch(entry.id);
						}
					};
					const toggle = (event) => {
						event.preventDefault();
						event.stopPropagation();
						toggleBranch(entry.id);
					};
					return (0, react_jsx_runtime.jsxs)("div", {
						className: SubagentCatalogAction_module_css_default.node,
						children: [(0, react_jsx_runtime.jsxs)("div", {
							role: "treeitem",
							tabIndex: 0,
							"aria-level": level,
							...knownLeaf ? {} : { "aria-expanded": isExpanded },
							className: SubagentCatalogAction_module_css_default.row,
							onClick: open,
							onKeyDown: handleKey,
							children: [knownLeaf ? reserveDisclosure && (0, react_jsx_runtime.jsx)("span", { className: SubagentCatalogAction_module_css_default.disclosureSpace }) : (0, react_jsx_runtime.jsx)("button", {
								type: "button",
								tabIndex: -1,
								className: `${SubagentCatalogAction_module_css_default.disclosure} ${isExpanded ? SubagentCatalogAction_module_css_default.disclosureOpen : ""}`,
								"aria-label": t(isExpanded ? "branch.collapse" : "branch.expand", { label }),
								onClick: toggle,
								children: (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconChevronRightOutline14, {})
							}), (0, react_jsx_runtime.jsxs)("div", {
								className: SubagentCatalogAction_module_css_default.clickarea,
								children: [
									(0, react_jsx_runtime.jsx)(ChildProgressContent, { parentSessionId, entry, label, mode, loadProgress, t }),
									metrics !== "" && (0, react_jsx_runtime.jsxs)("span", {
										className: SubagentCatalogAction_module_css_default.metrics,
										children: [tokenMetric !== void 0 && (0, react_jsx_runtime.jsx)("span", {
											className: SubagentCatalogAction_module_css_default.metricToken,
											children: tokenMetric
										}), durationMetric !== void 0 && (0, react_jsx_runtime.jsx)("span", {
											className: SubagentCatalogAction_module_css_default.metricDuration,
											title: t("duration.exactTitle", { duration: durationMetric.exact }),
											children: durationMetric.compact
										})]
									})
								]
							})]
						}), isExpanded && !knownLeaf && (0, react_jsx_runtime.jsx)("div", {
							role: "group",
							className: SubagentCatalogAction_module_css_default.children,
							"aria-busy": childLoading || void 0,
							children: childCatalog === void 0 ? (0, react_jsx_runtime.jsx)(CatalogLoadingRows, {
								parentSessionId: entry.id,
								summaries,
								level: level + 1,
								t
							}) : (0, react_jsx_runtime.jsx)(CatalogRows, {
								parentSessionId: entry.id,
								catalog: childCatalog,
								catalogs,
								summaries,
								expanded,
								level: level + 1,
								now,
								openChild,
									refresh,
									loadProgress,
								toggleBranch,
								closeCatalog,
								t
							})
						})]
					}, entry.id);
				})
			] });
		}
		/**
		* Render the current session's direct catalog and lazily expanded descendants.
		* @param props - session standard props plus catalog navigation actions.
		* @returns The action while the catalog is pending or summaries establish descendants.
		*/
		function SubagentCatalogAction({ sessionId, useSessions, openChild, refresh, loadProgress, setCatalogOpen, t }) {
			const catalogs = useSessions((state) => state.subagentsByParent);
			const summaries = useSessions((state) => state.byId);
			const catalog = catalogs[sessionId];
			const [open, setOpen] = (0, react.useState)(false);
			const [now, setNow] = (0, react.useState)(() => Date.now());
			const [menuTop, setMenuTop] = (0, react.useState)(96);
			const [expanded, setExpanded] = (0, react.useState)(() => /* @__PURE__ */ new Set());
			const rootRef = (0, react.useRef)(null);
			const triggerRef = (0, react.useRef)(null);
			const observedCatalogs = (0, react.useRef)(/* @__PURE__ */ new Set());
			const setCatalogOpenRef = (0, react.useRef)(setCatalogOpen);
			setCatalogOpenRef.current = setCatalogOpen;
			const healthy = catalog?.entries.filter((entry) => entry.kind === "child") ?? [];
			const descendants = (0, react.useMemo)(() => (0, _deepseek_ai_dsh_client_runtime_client.indexSubagentDescendants)(summaries).get(sessionId) ?? NO_DESCENDANTS, [sessionId, summaries]);
			const descendantCount = Math.max(healthy.length, descendants.count);
			const totalCountKey = descendantCount === 1 ? "count.total.one" : "count.total.other";
			const runningCountKey = descendants.runningCount === 1 ? "count.running.one" : "count.running.other";
			const presentedCatalog = descendants.count > 0 && (catalog === void 0 || catalog.state === "ready" && catalog.entries.length === 0) ? {
				entries: [],
				parentAvailable: catalog?.parentAvailable ?? false,
				state: "loading",
				error: null
			} : catalog;
			const observeCatalog = (parentSessionId, next) => {
				if (next) observedCatalogs.current.add(parentSessionId);
				else observedCatalogs.current.delete(parentSessionId);
				setCatalogOpen(parentSessionId, next);
			};
			const closeAllCatalogs = () => {
				for (const parentSessionId of observedCatalogs.current) setCatalogOpen(parentSessionId, false);
				observedCatalogs.current.clear();
				setExpanded(/* @__PURE__ */ new Set());
			};
			const changeOpen = (next, restoreFocus = false) => {
				setOpen(next);
				if (next) {
					setNow(Date.now());
					setMenuTop(Math.max(16, Math.min(window.innerHeight - 96, (triggerRef.current?.getBoundingClientRect().bottom ?? 91) + 5)));
					observeCatalog(sessionId, true);
				} else closeAllCatalogs();
				if (restoreFocus) queueMicrotask(() => {
					triggerRef.current?.focus();
				});
			};
			const closeBranch = (root) => {
				const closing = /* @__PURE__ */ new Set();
				const visit = (parentSessionId) => {
					if (closing.has(parentSessionId) || !expanded.has(parentSessionId)) return;
					closing.add(parentSessionId);
					const branch = catalogs[parentSessionId];
					for (const entry of branch?.entries ?? []) if (entry.kind === "child") visit(entry.id);
				};
				visit(root);
				for (const parentSessionId of closing) observeCatalog(parentSessionId, false);
				setExpanded((current) => new Set([...current].filter((id) => !closing.has(id))));
			};
			const toggleBranch = (childSessionId) => {
				if (expanded.has(childSessionId)) {
					closeBranch(childSessionId);
					return;
				}
				setExpanded((current) => new Set(current).add(childSessionId));
				observeCatalog(childSessionId, true);
			};
			(0, react.useEffect)(() => {
				if (!open) return;
				const closeOutside = (event) => {
					if (event.target instanceof Node && !rootRef.current?.contains(event.target)) changeOpen(false);
				};
				document.addEventListener("pointerdown", closeOutside);
				return () => {
					document.removeEventListener("pointerdown", closeOutside);
				};
			}, [open]);
			(0, react.useEffect)(() => {
				if (!open || descendants.runningCount === 0) return;
				const timer = setInterval(() => {
					setNow(Date.now());
				}, 1e3);
				return () => {
					clearInterval(timer);
				};
			}, [open, descendants.runningCount]);
			(0, react.useEffect)(() => () => {
				for (const parentSessionId of observedCatalogs.current) setCatalogOpenRef.current(parentSessionId, false);
				observedCatalogs.current.clear();
			}, []);
			const visible = presentedCatalog !== void 0 && (presentedCatalog.state === "error" || presentedCatalog.entries.length > 0 || descendantCount > 0);
			(0, react.useEffect)(() => {
				if (visible || !open) return;
				setOpen(false);
				closeAllCatalogs();
			}, [visible, open]);
			if (!visible) return null;
			const focusAt = (index) => {
				const items = treeItems(rootRef.current);
				if (items.length === 0) return;
				items[(index + items.length) % items.length]?.focus();
			};
			const navigate = (event) => {
				const items = treeItems(rootRef.current);
				const index = items.indexOf(document.activeElement);
				if (event.key === "Escape") {
					event.preventDefault();
					changeOpen(false, true);
				} else if (event.key === "Home") {
					event.preventDefault();
					focusAt(0);
				} else if (event.key === "End") {
					event.preventDefault();
					focusAt(items.length - 1);
				} else if (event.key === "ArrowDown") {
					event.preventDefault();
					focusAt(index + 1);
				} else if (event.key === "ArrowUp") {
					event.preventDefault();
					focusAt(index < 0 ? items.length - 1 : index - 1);
				}
			};
			return (0, react_jsx_runtime.jsxs)("div", {
				className: SubagentCatalogAction_module_css_default.root,
				style: { "--dsh-subagent-menu-top": `${menuTop}px` },
				ref: rootRef,
				onKeyDown: navigate,
				children: [(0, react_jsx_runtime.jsxs)("button", {
					ref: triggerRef,
					type: "button",
					className: SubagentCatalogAction_module_css_default.trigger,
					"aria-haspopup": "tree",
					"aria-expanded": open,
					"aria-label": t(descendants.runningCount > 0 ? runningCountKey : totalCountKey, { count: descendants.runningCount > 0 ? descendants.runningCount : descendantCount }),
					onClick: () => {
						changeOpen(!open);
					},
					onKeyDown: (event) => {
						if (event.key !== "ArrowDown") return;
						event.preventDefault();
						if (!open) changeOpen(true);
						queueMicrotask(() => {
							focusAt(0);
						});
					},
					children: [
						(0, react_jsx_runtime.jsx)("span", {
							className: SubagentCatalogAction_module_css_default.activitySlot,
							children: descendants.runningCount > 0 && (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, { state: "ongoing" })
						}),
						(0, react_jsx_runtime.jsx)("span", {
							className: SubagentCatalogAction_module_css_default.count,
							children: t(totalCountKey, { count: descendantCount })
						}),
						(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconChevronDownOutline14, { className: open ? SubagentCatalogAction_module_css_default.triggerOpen : void 0 })
					]
				}), open && (0, react_jsx_runtime.jsx)("div", {
					className: SubagentCatalogAction_module_css_default.menu,
					role: "tree",
					"aria-label": t("tree.aria"),
					children: (0, react_jsx_runtime.jsx)(CatalogRows, {
						parentSessionId: sessionId,
						catalog: presentedCatalog,
						catalogs,
						summaries,
						expanded,
						level: 1,
						now,
						openChild,
						refresh,
						loadProgress,
						toggleBranch,
						closeCatalog: () => {
							changeOpen(false);
						},
						t
					})
				})]
			});
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-subagent\src\client\SubagentReadOnlyComposer.module.css.mjs
		const css = ".h8-yzW_frame{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-layer-1);min-height:54px;color:var(--dsw-alias-label-tertiary);border-radius:14px;justify-content:center;align-items:center;gap:8px;margin:0 24px 20px;padding:10px 16px;font-size:13px;line-height:20px;display:flex}.h8-yzW_frame strong{color:var(--dsw-alias-label-primary);font-weight:510}";
		const tagId = "@deepseek-ai/dsh-client-ui-subagent/SubagentReadOnlyComposer.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-subagent";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var SubagentReadOnlyComposer_module_css_default = { "frame": "h8-yzW_frame" };
		//#endregion
		//#region lib/types/client/SubagentReadOnlyComposer.js
		/**
		* Explain why the normal composer is unavailable for an addressed child.
		* @param props - selector-owned read-only reason plus standard slot props.
		* @returns A read-only composer replacement.
		*/
		function SubagentReadOnlyComposer({ matched, t }) {
			const oneShot = matched.reason === "one-shot";
			return (0, react_jsx_runtime.jsxs)("div", {
				className: SubagentReadOnlyComposer_module_css_default.frame,
				role: "status",
				children: [(0, react_jsx_runtime.jsx)("strong", { children: t(oneShot ? "readonly.oneShot.title" : "readonly.title") }), (0, react_jsx_runtime.jsx)("span", { children: t(oneShot ? "readonly.oneShot.body" : "readonly.body") })]
			});
		}
		//#endregion
		//#region lib/types/client/locales.js
		/** `subagent` namespace dictionaries. */
		/** Dictionary namespace owned by this plugin. */
		const NS = "subagent";
		/** Simplified Chinese dictionary (the key-set source of truth). */
		const zh = {
            "team.title": "协作",
            "team.advancedDefaults": "临时子代理设置", "team.advancedHint": "临时委派不要求开启团队协作；是否可用由当前智能体的工具与权限决定。协作方案中的明确角色配置优先。",
            "team.stopAll": "停止全部", "team.taskStatus": "任务状态",
            "team.settingsTitle": "协作设置",
            "team.enable": "启用协作功能",
            "team.settingsDescription": "统一管理任务、成员和独立对话。方案设置保存后即时生效，已启动成员保留原配置。",
            "team.disabled": "协作功能尚未启用，请在“设置”页开启并保存。",
            "team.noTasks": "暂无任务。可以直接创建任务，再分配给成员。",
            "team.empty": "暂无成员。开启当前会话的协作后，可创建成员并分配工作。",
            "team.tasks": "任务",
            "team.messages": "消息",
            "team.settings": "设置",
            "team.mode": "当前方式",
            "team.mode.off": "单人",
            "team.mode.auto": "自动协作",
            "team.mode.custom": "自定义方案",
            "team.profile": "协作方案",
            "team.refresh": "刷新",
            "team.mainConversation": "主对话",
            "team.newConversation": "新建独立主对话",
            "team.configDuringRun": "执行期间保留当前配置；停止或完成后可更改协作方式。",
            "team.openConversation": "打开对话",
            "team.stopMember": "停止成员",
            "team.inheritModel": "沿用主对话模型",
            "team.otherMembers": "临时子代理",
            "team.standaloneHint": "主智能体可以按需临时委派，无需创建团队。下方可查看子代理记录，包含团队成员；可继续的子代理支持独立续聊，一次性子代理保留执行记录。",
            "team.noMessages": "暂无成员消息。",
            "team.delivered": "已投递",
            "team.queued": "等待投递",
            "team.enableSession": "选择自动协作或自定义方案后，可以创建成员。",
            "team.addMember": "创建成员",
            "team.memberName": "成员名称",
            "team.role": "角色",
            "team.initialTask": "首次任务",
            "team.createAndRun": "创建并执行",
            "team.memberMessage": "给此成员补充要求",
            "team.send": "发送",
            "team.editTask": "编辑任务",
            "team.addTask": "创建任务",
            "team.subject": "任务名称",
            "team.description": "任务内容",
            "team.acceptance": "验收条件",
            "team.owner": "执行成员",
            "team.result": "结果与验收证据",
            "team.save": "保存",
            "team.cancel": "取消",
            "team.edit": "编辑",
            "team.dispatch": "派发",
            "team.accept": "验收完成",
            "team.delete": "删除",
            "team.showButton": "显示输入区协作按钮",
            "team.defaultMode": "新会话默认方式",
            "team.defaultProfile": "默认方案",
            "team.none": "未选择",
            "team.profileName": "方案名称",
            "team.roleName": "角色名称",
            "team.instructions": "职责与要求",
            "team.model": "模型",
            "team.effort": "推理等级（留空沿用默认）",
            "team.maxTokens": "单次最大输出（留空沿用默认）",
            "team.tools": "可用工具，逗号分隔；留空继承",
            "team.canSpawn": "允许创建下级成员",
            "team.deleteRole": "删除角色",
            "team.addRole": "添加角色",
            "team.newRole": "新角色",
            "team.newProfile": "新方案",
            "team.addProfile": "添加方案",
            "team.copy": "副本",
            "team.reset": "重置未保存修改",
            "team.saved": "设置已保存并生效。",
            "team.unavailable": "当前连接不能保存协作设置。",
            "team.modelUnavailable": "目录中不可用",
            "team.writeScopes": "协作文件范围（逗号分隔）",
            "team.status.queued": "已派发",
            "team.status.review": "待验收",
            "team.status.blocked": "受阻",
            "team.status.cancelled": "已取消",

            "team.limit": "成员上限",
            "team.restart": "设置已保存，重启应用后生效。",
            "team.prepare": "填写团队任务",
            "team.template": "请组建团队完成以下任务，先明确各成员职责和文件范围，维护共享任务并汇总结果：\n",
            "team.pending": "{count} 条团队消息等待投递",
            "team.intro": "团队成员可并行处理独立任务；点击成员名称查看其对话。",

            "team.members": "成员",
            "team.lead": "负责人",
            "team.unassigned": "未分配",
            "team.close": "关闭",
            "team.dependencies": "前置任务",
            "team.scopeNote": "任务分配和文件范围用于协作，不会锁定工作区文件。",
            "team.status.provisioning": "正在创建",
            "team.status.active": "已就绪",
            "team.status.failed": "创建失败",
            "team.status.running": "运行中",
            "team.status.idle": "空闲",
            "team.status.inactive": "未运行",
            "team.status.pending": "待处理",
            "team.status.in_progress": "进行中",
            "team.status.completed": "已完成",

			"diagnostic.corrupt": "会话记录损坏",
			"diagnostic.unsupported": "旧版子代理记录（只读）",
			"diagnostic.unavailable": "会话记录暂不可用",
			"duration.seconds": "{seconds}秒",
			"duration.minutes": "{minutes}分{seconds}秒",
			"duration.hours": "{hours}小时{minutes}分{seconds}秒",
			"duration.days": "{days}天",
			"duration.daysHours": "{days}天{hours}小时",
			"duration.months": "约{months}个月",
			"duration.monthsDays": "约{months}个月{days}天",
			"duration.years": "约{years}年",
			"duration.yearsMonths": "约{years}年{months}个月",
			"duration.exactDays": "{days}天{hours}小时{minutes}分{seconds}秒",
			"duration.exactTitle": "总活跃耗时：{duration}",
			"loading.label": "正在加载子代理…",
			"loading.aria": "正在加载子代理",
			"load.error": "无法加载子代理",
			"retry": "重试",
			"mode.oneShot": "一次性",
			"mode.continuable": "可继续",
			"activity.running": "正在运行",
			"activity.inactive": "当前未运行",
			"progress.running": "正在运行",
			"progress.completed": "已完成",
			"progress.failed": "执行失败",
			"progress.stopped": "已停止",
			"progress.blocked": "需要处理",
			"progress.inactive": "当前未运行",
			"progress.unavailable": "状态暂不可用",
			"tool.title": "子任务",
			"tool.open": "打开子任务",
			"tool.details": "调用详情",
			"copy": "复制",
			"copied": "已复制",
			"branch.collapse": "收起 {label} 的下级子代理",
			"branch.expand": "展开 {label} 的下级子代理",
			"count.total.one": "{count} 个子代理",
			"count.total.other": "{count} 个子代理",
			"count.running.one": "{count} 个子代理，正在运行",
			"count.running.other": "{count} 个子代理，正在运行",
			"tree.aria": "子代理会话",
			"readonly.oneShot.title": "一次性子代理记录",
			"readonly.title": "此子代理暂时只读",
			"readonly.oneShot.body": "一次性任务不支持后续消息，可在这里查看完整执行记录。",
			"readonly.body": "父会话当前不在线，重新打开父会话后即可继续发送消息。"
		};
		/** English dictionary, key-identical to the Chinese source of truth. */
		const en = {
            "team.title": "Collaboration",
            "team.advancedDefaults": "Ad hoc subagent settings", "team.advancedHint": "Ad hoc delegation does not require team collaboration. Availability follows the current agent's tools and permissions. Explicit role settings in a collaboration profile take precedence.",
            "team.stopAll": "Stop all", "team.taskStatus": "Task status",
            "team.settingsTitle": "Collaboration settings",
            "team.enable": "Enable collaboration",
            "team.settingsDescription": "Manage tasks, members and their conversations together. Saved defaults apply immediately; existing members retain their configuration.",
            "team.disabled": "Enable collaboration in the Settings tab and save.",
            "team.noTasks": "No tasks. Create a task and assign a member.",
            "team.empty": "No members. Enable collaboration for this conversation to create members.",
            "team.tasks": "Tasks",
            "team.messages": "Messages",
            "team.settings": "Settings",
            "team.mode": "Mode",
            "team.mode.off": "Solo",
            "team.mode.auto": "Automatic",
            "team.mode.custom": "Profile",
            "team.profile": "Collaboration profile",
            "team.refresh": "Refresh",
            "team.mainConversation": "Main conversation",
            "team.newConversation": "New main conversation",
            "team.configDuringRun": "The current configuration is retained during execution. Change it after stopping or completing work.",
            "team.openConversation": "Open conversation",
            "team.stopMember": "Stop member",
            "team.inheritModel": "Use main conversation model",
            "team.otherMembers": "Ad hoc subagents",
            "team.standaloneHint": "The main agent can delegate as needed without creating a team. Browse subagent records below, including team members. Continuable subagents support follow-up conversations; one-shot subagents retain their execution history.",
            "team.noMessages": "No member messages.",
            "team.delivered": "Delivered",
            "team.queued": "Queued",
            "team.enableSession": "Select Automatic or a profile to create members.",
            "team.addMember": "Create member",
            "team.memberName": "Member name",
            "team.role": "Role",
            "team.initialTask": "Initial task",
            "team.createAndRun": "Create and run",
            "team.memberMessage": "Message this member",
            "team.send": "Send",
            "team.editTask": "Edit task",
            "team.addTask": "Create task",
            "team.subject": "Task title",
            "team.description": "Description",
            "team.acceptance": "Acceptance criteria",
            "team.owner": "Assignee",
            "team.result": "Result and evidence",
            "team.save": "Save",
            "team.cancel": "Cancel",
            "team.edit": "Edit",
            "team.dispatch": "Dispatch",
            "team.accept": "Accept result",
            "team.delete": "Delete",
            "team.showButton": "Show composer collaboration button",
            "team.defaultMode": "Default mode",
            "team.defaultProfile": "Default profile",
            "team.none": "None",
            "team.profileName": "Profile name",
            "team.roleName": "Role name",
            "team.instructions": "Responsibilities",
            "team.model": "Model",
            "team.effort": "Reasoning effort (empty uses default)",
            "team.maxTokens": "Maximum output tokens (empty uses default)",
            "team.tools": "Allowed tools, comma-separated; empty inherits",
            "team.canSpawn": "Allow delegation",
            "team.deleteRole": "Delete role",
            "team.addRole": "Add role",
            "team.newRole": "New role",
            "team.newProfile": "New profile",
            "team.addProfile": "Add profile",
            "team.copy": "Copy",
            "team.reset": "Reset unsaved changes",
            "team.saved": "Settings saved and applied.",
            "team.unavailable": "Collaboration settings cannot be saved on this connection.",
            "team.modelUnavailable": "Unavailable in catalog",
            "team.writeScopes": "Coordinated file scopes (comma-separated)",
            "team.status.queued": "Dispatched",
            "team.status.review": "Awaiting review",
            "team.status.blocked": "Blocked",
            "team.status.cancelled": "Cancelled",

            "team.limit": "Member limit",
            "team.restart": "Saved. Restart the application to apply.",
            "team.prepare": "Draft a team task",
            "team.template": "Create a team for the following task. Define each member’s responsibilities and file scope, maintain shared tasks, and summarize the results:\n",
            "team.pending": "{count} team messages awaiting delivery",
            "team.intro": "Teammates can work on independent tasks in parallel. Select a member to open their conversation.",

            "team.members": "Members",
            "team.lead": "Lead",
            "team.unassigned": "Unassigned",
            "team.close": "Close",
            "team.dependencies": "Dependencies",
            "team.scopeNote": "Task assignments and file scopes coordinate work; they do not lock workspace files.",
            "team.status.provisioning": "Creating",
            "team.status.active": "Ready",
            "team.status.failed": "Creation failed",
            "team.status.running": "Running",
            "team.status.idle": "Idle",
            "team.status.inactive": "Inactive",
            "team.status.pending": "Pending",
            "team.status.in_progress": "In progress",
            "team.status.completed": "Completed",

			"diagnostic.corrupt": "corrupted session record",
			"diagnostic.unsupported": "legacy subagent record (read-only)",
			"diagnostic.unavailable": "session record temporarily unavailable",
			"duration.seconds": "{seconds}s",
			"duration.minutes": "{minutes}m {seconds}s",
			"duration.hours": "{hours}h {minutes}m {seconds}s",
			"duration.days": "{days}d",
			"duration.daysHours": "{days}d {hours}h",
			"duration.months": "~{months}mo",
			"duration.monthsDays": "~{months}mo {days}d",
			"duration.years": "~{years}y",
			"duration.yearsMonths": "~{years}y {months}mo",
			"duration.exactDays": "{days}d {hours}h {minutes}m {seconds}s",
			"duration.exactTitle": "Total active duration: {duration}",
			"loading.label": "Loading subagents…",
			"loading.aria": "Loading subagents",
			"load.error": "Unable to load subagents",
			"retry": "Retry",
			"mode.oneShot": "one-shot",
			"mode.continuable": "continuable",
			"activity.running": "running",
			"activity.inactive": "not running",
			"progress.running": "Running",
			"progress.completed": "Completed",
			"progress.failed": "Failed",
			"progress.stopped": "Stopped",
			"progress.blocked": "Needs attention",
			"progress.inactive": "Not running",
			"progress.unavailable": "Status unavailable",
			"tool.title": "Subtask",
			"tool.open": "Open subtask",
			"tool.details": "Call details",
			"copy": "Copy",
			"copied": "Copied",
			"branch.collapse": "Collapse {label} descendants",
			"branch.expand": "Expand {label} descendants",
			"count.total.one": "{count} subagent",
			"count.total.other": "{count} subagents",
			"count.running.one": "{count} subagent running",
			"count.running.other": "{count} subagents running",
			"tree.aria": "Subagent sessions",
			"readonly.oneShot.title": "One-shot subagent record",
			"readonly.title": "This subagent is read-only for now",
			"readonly.oneShot.body": "One-shot tasks do not accept follow-ups; review the full execution record here.",
			"readonly.body": "The parent session is offline; reopen it to continue sending messages."
		};
		//#endregion
		//#region lib/types/client/index.js
		/** Required services for references, conversation slots, and session navigation. */
		const inject = [
			"settingsScope",
			"conversation",
			"inputTriggers",
			"connection",
			"sessions",
			"slots",
			"locale"
		];
		/** Claim the composer for one-shot history or an unavailable continuation owner. */
		function selectReadOnlySubagent(owner) {
			const subagent = owner.session?.subagent;
			if (subagent === void 0 || subagent === null) return null;
			if (subagent.address.mode === "one-shot") return { reason: "one-shot" };
			if (subagent.parentAvailable) return null;
			return owner.session?.running === true ? null : { reason: "parent-unavailable" };
		}
		/**
		* Client plugin body: register the '@' subagent source over the root session list.
		* @param ctx - client root context.
		*/
        function TeamBoardAction(props) {
            const {parentSessionId,openChild,openMain,createMain,teamSettings,saveSettings,loadModels,renderDefaults,refreshList,panel,t}=props;
            const h=react.createElement;
            const [fallback]=react.useState(createTeamPanel);
            const controller=panel??fallback;
            const panelState=react.useSyncExternalStore(controller.subscribe,controller.getSnapshot);
            const open=panelState.openFor===parentSessionId;
            const [state,setState]=react.useState(null),[error,setError]=react.useState(null),[busy,setBusy]=react.useState(false),[tab,setTab]=react.useState('tasks'),[refresh,setRefresh]=react.useState(0);
            const active=react.useRef(parentSessionId),epoch=react.useRef(0),pending=react.useRef(false),operationError=react.useRef(null),trigger=react.useRef(null);
            active.current=parentSessionId;
            const scope=useTeamScope(teamSettings);
            react.useEffect(()=>{epoch.current++;operationError.current=null;setState(null);setError(null);setBusy(false);pending.current=false;return()=>controller.close(parentSessionId);},[parentSessionId,controller]);
            react.useEffect(()=>{
                const abort=new AbortController();let alive=true,timer;
                const load=async()=>{
                    if(pending.current||(open&&document.visibilityState==='hidden')){timer=setTimeout(load,2000);return;}
                    const generation=epoch.current;
                    try{
                        const value=await teamRequest(parentSessionId,null,abort.signal);
                        if(alive&&generation===epoch.current){setState(value);controller.update(parentSessionId,value);setError(operationError.current);}
                    }catch(cause){if(alive&&!abort.signal.aborted)setError(String(cause.message||cause));}
                    finally{if(alive&&open)timer=setTimeout(load,2000);}
                };
                load();return()=>{alive=false;abort.abort();clearTimeout(timer);};
            },[parentSessionId,open,refresh,scope?.value,controller]);
            const board=state?.board,enabled=state?.enabled===true,members=Object.values(board?.members??{}),tasks=Object.values(board?.tasks??{}).filter(task=>task.status!=='deleted');
            const config=board?.config??{revision:0,mode:'off'},profiles=state?.settings?.profiles??scope?.value?.profiles??[];
            const configurable=enabled&&!busy&&!board?.leadRunning&&!members.some(m=>m.status==='running'||m.phase==='provisioning');
            const mutate=async args=>{
                if(pending.current)return false;
                pending.current=true;operationError.current=null;setBusy(true);setError(null);epoch.current++;
                const owner=parentSessionId;
                try{
                    const value=await teamRequest(board?.teamId??owner,args);
                    if(refreshList)void refreshList().catch(()=>{});
                    if(active.current===owner){setState(value);controller.update(owner,value);if(value.board?.receipt?.error)setError(value.board.receipt.error);}
                    return true;
                }catch(cause){if(active.current===owner){operationError.current=String(cause.message||cause);setError(operationError.current);}return false;}
                finally{if(active.current===owner){pending.current=false;setBusy(false);setRefresh(v=>v+1);}}
            };
            const close=()=>{controller.close(parentSessionId);trigger.current?.focus();};
            const label=id=>id===board?.teamId?t('team.lead'):members.find(m=>m.id===id)?.description??t('team.unassigned');
            const configure=(mode,profileId=config.profile?.id??'')=>mutate({action:'configure',mode,profileId:mode==='custom'?profileId:'',expectedRevision:config.revision??0});
            return h(react.Fragment,null,
                !props.panelOnly&&h('button',{ref:trigger,type:'button',className:'dshTeamTrigger','aria-label':t('team.title'),'aria-haspopup':'dialog','aria-expanded':open,onClick:()=>controller.open(parentSessionId)},teamIcon(),t('team.title'),members.length>0&&h('span',null,members.length)),
                open&&h(_deepseek_ai_dsh_client_ui_primitives.Modal,{open:true,title:t('team.title'),className:'dshTeamModal',contentClassName:'dshTeamModalContent',closeLabel:t('team.close'),onClose:close},
                    h('div',{'data-agent-team-board':true,className:'dshCollaborationPanel'},
                        error&&h('p',{role:'alert',className:'dshTeamError'},error),
                        h('div',{className:'dshTeamToolbar'},
                            h('label',null,t('team.mode'),h('select',{'aria-label':t('team.mode'),value:config.mode,disabled:!configurable,onChange:e=>configure(e.target.value,config.profile?.id??profiles[0]?.id??'')},
                                ['off','auto','custom'].map(mode=>h('option',{key:mode,value:mode,disabled:mode==='custom'&&!profiles.length},t('team.mode.'+mode))))),
                            config.mode==='custom'&&h('select',{'aria-label':t('team.profile'),value:config.profile?.id??'',disabled:!configurable,onChange:e=>configure('custom',e.target.value)},profiles.map(profile=>h('option',{key:profile.id,value:profile.id},profile.name))),
                            h('button',{type:'button',className:'dshTeamButton',disabled:busy,onClick:()=>{operationError.current=null;setError(null);setRefresh(v=>v+1);}},t('team.refresh')),
                            h('button',{type:'button',className:'dshTeamButton',disabled:busy||!board?.leadRunning&&!members.some(m=>m.status==='running'||m.phase==='provisioning'),onClick:()=>mutate({action:'stopAll'})},t('team.stopAll')),
                            board?.teamId!==parentSessionId&&h('button',{type:'button',className:'dshTeamButton',onClick:()=>{close();openMain?.(board.teamId);}},t('team.mainConversation')),
                            createMain&&h('button',{type:'button',className:'dshTeamButton',disabled:busy||!enabled,onClick:async()=>{setError(null);setBusy(true);try{await createMain(config);close();}catch(cause){setError(String(cause.message||cause));}finally{setBusy(false);}}},t('team.newConversation'))),
                        !configurable&&enabled&&h('p',{className:'dshTeamHint'},t('team.configDuringRun')),
                        h('div',{role:'tablist',className:'dshTeamTabs'},['tasks','members','messages','settings'].map(id=>h('button',{key:id,type:'button',role:'tab','aria-selected':tab===id,onClick:()=>setTab(id)},t('team.'+id)))),
                        !enabled&&tab!=='settings'&&h('p',{className:'dshTeamHint'},t('team.disabled')),
                        tab==='tasks'&&h('div',{role:'tabpanel'},
                            enabled&&h('details',{className:'dshTeamCreate'},h('summary',null,t('team.addTask')),h(TeamTaskForm,{key:board?.teamId,members,tasks,busy,t,onSave:mutate})),
                            tasks.length===0&&h('p',{className:'dshTeamHint'},t('team.noTasks')),
                            tasks.map(task=>h(TeamTaskCard,{key:task.id,task,tasks,members,label,busy,enabled,t,onSave:mutate}))),
                        tab==='members'&&h('div',{role:'tabpanel'},
                            h('section',{'data-standalone-subagents':true,className:'dshTeamCard'},h('h3',null,t('team.otherMembers')),h('p',{className:'dshTeamHint'},t('team.standaloneHint')),props.useSessions&&h(SubagentCatalogAction,{...props,sessionId:parentSessionId})),
                            enabled&&config.mode!=='off'&&h('details',{className:'dshTeamCreate'},h('summary',null,t('team.addMember')),h(TeamMemberForm,{key:config.revision,config,busy,t,onSave:mutate})),
                            enabled&&config.mode==='off'&&h('p',{className:'dshTeamHint'},t('team.enableSession')),
                            members.length===0&&h('p',{className:'dshTeamHint'},t('team.empty')),
                            members.map(member=>h('article',{key:member.id,className:'dshTeamCard'},
                                h('div',{className:'dshTeamToolbar'},h('strong',null,member.description||member.name),h('span',null,t('team.status.'+(member.status??member.phase))),
                                    h('button',{type:'button',className:'dshTeamButton',disabled:member.phase!=='active',onClick:()=>{close();openChild({parentSessionId:board.teamId,childSessionId:member.id,mode:'continuable'});}},t('team.openConversation')),
                                    h('button',{type:'button',className:'dshTeamButton',disabled:busy||member.status!=='running',onClick:()=>mutate({action:'interrupt',target:member.id})},t('team.stopMember'))),
                                member.role&&h('p',{className:'dshTeamHint'},`${member.role.name} · ${member.role.provider&&member.role.model?member.role.provider+' / '+member.role.model:t('team.inheritModel')}`),
                                member.error&&h('p',{role:'status',className:'dshTeamError'},member.error),
                                member.phase==='active'&&h(TeamMessageForm,{target:member.id,busy:busy||!enabled,t,onSend:mutate})))),
                        tab==='messages'&&h('div',{role:'tabpanel'},
                            !(board?.mailbox?.length)&&h('p',{className:'dshTeamHint'},t('team.noMessages')),
                            (board?.mailbox??[]).map(mail=>h('article',{key:mail.id,className:'dshTeamCard'},h('strong',null,`${mail.sender} → ${label(mail.targetId)}`),h('p',null,(mail.content??[]).filter(c=>c.type==='text').map(c=>c.text).join('\n')),h('span',{className:'dshTeamHint'},t(mail.cancelled?'team.status.cancelled':mail.delivered?'team.delivered':'team.queued'))))),
                        tab==='settings'&&h(TeamSettings,{scope:teamSettings,saveSettings,loadModels,renderDefaults,t})
                    )));
        }
        async function teamRequest(sessionId,args,signal) {
            const response=await fetch('/__dsh-agent-team',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId,...(args?{action:'control',arguments:args}:{})}),signal});
            const value=await response.json();if(!response.ok)throw new Error(value.error||`HTTP ${response.status}`);return value;
        }
        async function teamModelDirectory() {
            const response=await fetch('/task-models/describe',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:'{}'});
            const value=await response.json();if(!response.ok)throw new Error(value.error||`HTTP ${response.status}`);
            return {groups:value.providers??[]};
        }
        function teamIcon(){const h=react.createElement;return h('svg',{width:16,height:16,viewBox:'0 0 24 24',fill:'none',stroke:'currentColor',strokeWidth:1.6,'aria-hidden':true},h('path',{d:'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75'}),h('circle',{cx:9,cy:7,r:4}));}
        function createTeamPanel(){let state={openFor:null,views:{}};const listeners=new Set();const emit=next=>{state=next;for(const fn of listeners)fn();};return{getSnapshot:()=>state,subscribe:fn=>{listeners.add(fn);return()=>listeners.delete(fn);},open:id=>emit({...state,openFor:id}),close:id=>{if(state.openFor===id)emit({...state,openFor:null});},update:(id,value)=>emit({...state,views:{...state.views,[id]:value}})};}
        const EMPTY_TEAM_SCOPE={getSnapshot:()=>null,subscribe:()=>()=>{}};
        function useTeamScope(scope){const subscribe=react.useCallback(fn=>scope?scope.subscribe(fn):()=>{},[scope]),read=react.useCallback(()=>scope?.getSnapshot()??null,[scope]);return react.useSyncExternalStore(subscribe,read);}
        function TeamComposerTrigger({parentSessionId,panel,teamSettings,t,header=false}){
            const h=react.createElement,view=react.useSyncExternalStore(panel.subscribe,panel.getSnapshot).views[parentSessionId];
            const settings=useTeamScope(teamSettings);
            if(!header&&settings?.value?.showButton===false)return null;
            return h('button',{type:'button',className:'dshTeamTrigger','aria-label':t('team.title'),onClick:()=>panel.open(parentSessionId)},teamIcon(),header?t('team.title'):t('team.mode.'+(view?.board?.config?.mode??'off')));
        }
        function teamIdentity(prefix){return prefix+'-'+crypto.randomUUID().slice(0,12);}
        function TeamField({label,children}){return react.createElement('label',{className:'dshTeamField'},react.createElement('span',null,label),children);}
        function TeamMemberForm({config,busy,t,onSave}){
            const h=react.createElement,[title,setTitle]=react.useState(''),[prompt,setPrompt]=react.useState(''),[roleId,setRoleId]=react.useState(config.profile?.roles?.[0]?.id??''),[requestId,setRequestId]=react.useState(()=>teamIdentity('member'));
            const edit=(setter,value)=>{setter(value);setRequestId(teamIdentity('member'));};
            return h('form',{className:'dshTeamCard',onSubmit:async e=>{e.preventDefault();if(await onSave({action:'create',name:requestId,requestId,description:title.trim(),prompt:prompt.trim(),context:'fresh',...(roleId?{roleId}:{})})){setTitle('');setPrompt('');setRequestId(teamIdentity('member'));}}},
                h('fieldset',{disabled:busy},h('legend',null,t('team.addMember')),
                    h(TeamField,{label:t('team.memberName')},h('input',{required:true,maxLength:128,value:title,onChange:e=>edit(setTitle,e.target.value)})),
                    config.profile&&h(TeamField,{label:t('team.role')},h('select',{value:roleId,onChange:e=>edit(setRoleId,e.target.value)},config.profile.roles.map(role=>h('option',{key:role.id,value:role.id},role.name)))),
                    h(TeamField,{label:t('team.initialTask')},h('textarea',{required:true,rows:3,maxLength:16000,value:prompt,onChange:e=>edit(setPrompt,e.target.value)})),
                    h('button',{type:'submit',className:'dshTeamButton',disabled:!title.trim()||!prompt.trim()},t('team.createAndRun'))));
        }
        function TeamMessageForm({target,busy,t,onSend}){
            const h=react.createElement,[text,setText]=react.useState(''),[id,setId]=react.useState(()=>teamIdentity('message'));
            return h('form',{className:'dshTeamMessage',onSubmit:async e=>{e.preventDefault();if(await onSend({action:'message',target,message:text.trim(),messageId:id})){setText('');setId(teamIdentity('message'));}}},
                h('input',{'aria-label':t('team.memberMessage'),placeholder:t('team.memberMessage'),maxLength:8000,value:text,disabled:busy,onChange:e=>{setText(e.target.value);setId(teamIdentity('message'));}}),
                h('button',{type:'submit',className:'dshTeamButton',disabled:busy||!text.trim()},t('team.send')));
        }
        function TeamTaskForm({task,members,tasks,busy,t,onSave,onCancel}){
            const h=react.createElement,[draft,setDraft]=react.useState(()=>({subject:task?.subject??'',description:task?.description??'',acceptance:task?.acceptance??'',owner:task?.ownerId??'',blockedBy:task?.blockedBy??[],writeScopes:(task?.writeScopes??[]).join(', '),result:task?.result??'',status:task?.status??'pending'})),[id,setId]=react.useState(()=>task?.id??teamIdentity('task'));
            const field=(name,value)=>setDraft(previous=>({...previous,[name]:value}));
            return h('form',{className:'dshTeamCard',onSubmit:async e=>{e.preventDefault();if(await onSave({action:'task',taskId:id,expectedRevision:task?.revision??0,...draft,owner:draft.owner||null,writeScopes:draft.writeScopes.split(',').map(s=>s.trim()).filter(Boolean)})){if(onCancel)onCancel();else {setDraft({subject:'',description:'',acceptance:'',owner:'',blockedBy:[],writeScopes:'',result:'',status:'pending'});setId(teamIdentity('task'));}}}},
                h('fieldset',{disabled:busy},h('legend',null,t(task?'team.editTask':'team.addTask')),
                    ['subject','description','acceptance'].map(name=>h(TeamField,{key:name,label:t('team.'+name)},h(name==='subject'?'input':'textarea',{required:name==='subject',rows:2,value:draft[name],maxLength:name==='subject'?256:8000,onChange:e=>field(name,e.target.value)}))),
                    h(TeamField,{label:t('team.owner')},h('select',{value:draft.owner,onChange:e=>field('owner',e.target.value)},h('option',{value:''},t('team.unassigned')),members.filter(m=>m.phase==='active').map(m=>h('option',{key:m.id,value:m.id},m.description||m.name)))),
                    tasks.length>0&&h(TeamField,{label:t('team.dependencies')},h('select',{multiple:true,value:draft.blockedBy,onChange:e=>field('blockedBy',Array.from(e.target.selectedOptions,o=>o.value))},tasks.filter(item=>item.id!==task?.id).map(item=>h('option',{key:item.id,value:item.id},item.subject)))),
                    h(TeamField,{label:t('team.writeScopes')},h('input',{value:draft.writeScopes,onChange:e=>field('writeScopes',e.target.value)})),
                    task&&h(TeamField,{label:t('team.taskStatus')},h('select',{value:draft.status,onChange:e=>field('status',e.target.value)},['pending','review','blocked','completed','cancelled'].map(status=>h('option',{key:status,value:status},t('team.status.'+status))))),
                    task&&h(TeamField,{label:t('team.result')},h('textarea',{rows:3,value:draft.result,maxLength:16000,onChange:e=>field('result',e.target.value)})),
                    h('div',{className:'dshTeamToolbar'},h('button',{type:'submit',className:'dshTeamButton',disabled:!draft.subject.trim()},t('team.save')),onCancel&&h('button',{type:'button',className:'dshTeamButton',onClick:onCancel},t('team.cancel')))));
        }
        function TeamTaskCard({task,tasks,members,label,busy,enabled,t,onSave}){
            const h=react.createElement,[editing,setEditing]=react.useState(false),active=['queued','in_progress'].includes(task.status);
            if(editing)return h(TeamTaskForm,{key:task.id+':'+task.revision,task,tasks,members,busy,t,onSave,onCancel:()=>setEditing(false)});
            return h('article',{className:'dshTeamCard'},h('strong',null,task.subject),h('p',null,task.description),h('p',{className:'dshTeamHint'},`${label(task.ownerId)} · ${t('team.status.'+task.status)}`),
                task.acceptance&&h('p',null,t('team.acceptance')+': '+task.acceptance),task.result&&h('p',null,t('team.result')+': '+task.result),
                task.blockedBy.length>0&&h('p',{className:'dshTeamHint'},t('team.dependencies')+': '+task.blockedBy.map(id=>tasks.find(item=>item.id===id)?.subject??id).join(', ')),
                task.writeScopes.length>0&&h('p',{className:'dshTeamHint'},t('team.writeScopes')+': '+task.writeScopes.join(', ')),
                h('div',{className:'dshTeamToolbar'},
                    h('button',{type:'button',className:'dshTeamButton',disabled:busy||!enabled||active,onClick:()=>setEditing(true)},t('team.edit')),
                    h('button',{type:'button',className:'dshTeamButton',disabled:busy||!enabled||!task.ownerId||!['pending','blocked','cancelled'].includes(task.status)||task.blockedBy.some(id=>tasks.find(item=>item.id===id)?.status!=='completed'),onClick:()=>onSave({action:'dispatch',taskId:task.id,expectedRevision:task.revision})},t('team.dispatch')),
                    task.status==='review'&&h('button',{type:'button',className:'dshTeamButton',disabled:busy||!enabled||!!task.acceptance&&!task.result,onClick:()=>onSave({action:'task',taskId:task.id,expectedRevision:task.revision,status:'completed'})},t('team.accept')),
                    h('button',{type:'button',className:'dshTeamButton',disabled:busy||!enabled||active,onClick:()=>onSave({action:'task',taskId:task.id,expectedRevision:task.revision,status:'deleted'})},t('team.delete'))));
        }
        function TeamSettings({scope,saveSettings,loadModels,renderDefaults,t}) {
            const h=react.createElement,[snapshot,setSnapshot]=react.useState(()=>scope?.getSnapshot()),[draft,setDraft]=react.useState(null),[saving,setSaving]=react.useState(false),[error,setError]=react.useState(null),[saved,setSaved]=react.useState(false),[groups,setGroups]=react.useState([]);
            const dirty=react.useRef(false),base=react.useRef(null);
            react.useEffect(()=>{if(!scope)return;const adopt=()=>{const next=scope.getSnapshot();setSnapshot(next);if(!dirty.current){setDraft(structuredClone(next.value??{}));base.current=next.revision;}};adopt();return scope.subscribe(adopt);},[scope]);
            react.useEffect(()=>{let alive=true;if(loadModels)loadModels().then(value=>{if(alive)setGroups(value.groups??[]);}).catch(cause=>{if(alive)setError(String(cause.message||cause));});return()=>{alive=false;};},[loadModels]);
            const value=draft??snapshot?.value??{},profiles=value.profiles??[],unavailable=!snapshot?.writable||snapshot?.mode==='memory'||saving;
            const set=next=>{dirty.current=true;setSaved(false);setDraft(next);};
            const field=(key,next)=>set({...value,[key]:next});
            const updateProfile=(id,patch)=>field('profiles',profiles.map(p=>p.id===id?{...p,...patch}:p));
            const updateRole=(profileId,roleId,patch)=>updateProfile(profileId,{roles:profiles.find(p=>p.id===profileId).roles.map(r=>r.id===roleId?{...r,...patch}:r)});
            const blankRole=()=>({id:teamIdentity('role'),name:t('team.newRole'),instructions:'',provider:'',model:'',reasoningEffort:'',maxTokens:null,allowTools:[],canSpawn:false});
            const save=async()=>{if(unavailable)return;setSaving(true);setError(null);try{if(!saveSettings)throw Error(t('team.unavailable'));await saveSettings(value,base.current);dirty.current=false;await scope.load?.();setDraft(structuredClone(scope.getSnapshot().value??value));base.current=scope.getSnapshot().revision;setSaved(true);}catch(cause){setError(String(cause.message||cause));}finally{setSaving(false);}};
            return h('section',{className:'dshTeamSettings','data-team-settings':true},
                h('p',{className:'dshTeamHint'},t('team.settingsDescription')),
                [['enabled','team.enable'],['showButton','team.showButton']].map(([key,label])=>h('label',{key,className:'dshTeamSetting'},h('span',null,t(label)),h('input',{type:'checkbox',role:'switch',checked:key==='showButton'?value[key]!==false:value[key]===true,disabled:unavailable,onChange:e=>field(key,e.target.checked)}))),
                h('label',{className:'dshTeamSetting'},h('span',null,t('team.limit')),h('select',{value:value.maxMembers??8,disabled:unavailable,onChange:e=>field('maxMembers',Number(e.target.value))},Array.from({length:16},(_,i)=>h('option',{key:i,value:i+1},i+1)))),
                h('label',{className:'dshTeamSetting'},h('span',null,t('team.defaultMode')),h('select',{value:value.defaultMode??'off',disabled:unavailable,onChange:e=>set({...value,defaultMode:e.target.value,defaultProfile:e.target.value==='custom'?(value.defaultProfile||profiles[0]?.id||''):value.defaultProfile??''})},['off','auto','custom'].map(mode=>h('option',{key:mode,value:mode,disabled:mode==='custom'&&!profiles.length},t('team.mode.'+mode))))),
                h('label',{className:'dshTeamSetting'},h('span',null,t('team.defaultProfile')),h('select',{value:value.defaultProfile??'',disabled:unavailable,onChange:e=>field('defaultProfile',e.target.value)},h('option',{value:'',disabled:value.defaultMode==='custom'},t('team.none')),profiles.map(p=>h('option',{key:p.id,value:p.id},p.name)))),
                profiles.map(profile=>h('fieldset',{key:profile.id,className:'dshTeamCard',disabled:unavailable},
                    h('legend',null,t('team.profile')),h(TeamField,{label:t('team.profileName')},h('input',{value:profile.name,maxLength:128,onChange:e=>updateProfile(profile.id,{name:e.target.value})})),
                    profile.roles.map(role=>h('details',{key:role.id,className:'dshTeamRole'},h('summary',null,role.name||t('team.newRole')),
                        h(TeamField,{label:t('team.roleName')},h('input',{value:role.name,maxLength:128,onChange:e=>updateRole(profile.id,role.id,{name:e.target.value})})),
                        h(TeamField,{label:t('team.instructions')},h('textarea',{value:role.instructions??'',rows:3,maxLength:8000,onChange:e=>updateRole(profile.id,role.id,{instructions:e.target.value})})),
                        h(TeamField,{label:t('team.model')},h('select',{value:role.provider&&role.model?JSON.stringify([role.provider,role.model]):'',onChange:e=>{const [provider,model]=e.target.value?JSON.parse(e.target.value):['',''];updateRole(profile.id,role.id,{provider,model,reasoningEffort:''});}},h('option',{value:''},t('team.inheritModel')),groups.map(group=>h('optgroup',{key:group.id,label:group.name},group.models.map(model=>h('option',{key:model.id,value:JSON.stringify([group.id,model.id])},model.name||model.id)))),role.provider&&role.model&&!groups.some(group=>group.id===role.provider&&group.models.some(model=>model.id===role.model))&&h('option',{value:JSON.stringify([role.provider,role.model])},role.provider+' / '+role.model+' · '+t('team.modelUnavailable')))),
                        h(TeamField,{label:t('team.effort')},h('input',{value:role.reasoningEffort??'',maxLength:64,onChange:e=>updateRole(profile.id,role.id,{reasoningEffort:e.target.value})})),
                        h(TeamField,{label:t('team.maxTokens')},h('input',{type:'number',min:1,max:1000000,value:role.maxTokens??'',onChange:e=>updateRole(profile.id,role.id,{maxTokens:e.target.value?Number(e.target.value):null})})),
                        h(TeamField,{label:t('team.tools')},h('input',{value:(role.allowTools??[]).join(', '),onChange:e=>updateRole(profile.id,role.id,{allowTools:e.target.value.split(',').map(s=>s.trim()).filter(Boolean)})})),
                        h('label',{className:'dshTeamSetting'},h('span',null,t('team.canSpawn')),h('input',{type:'checkbox',role:'switch',checked:role.canSpawn===true,onChange:e=>updateRole(profile.id,role.id,{canSpawn:e.target.checked})})),
                        h('button',{type:'button',className:'dshTeamButton',disabled:profile.roles.length===1,onClick:()=>updateProfile(profile.id,{roles:profile.roles.filter(r=>r.id!==role.id)})},t('team.deleteRole')))),
                    h('div',{className:'dshTeamToolbar'},h('button',{type:'button',className:'dshTeamButton',disabled:profile.roles.length>=16,onClick:()=>updateProfile(profile.id,{roles:[...profile.roles,blankRole()]})},t('team.addRole')),
                        h('button',{type:'button',className:'dshTeamButton',disabled:profiles.length>=16,onClick:()=>field('profiles',[...profiles,{...structuredClone(profile),id:teamIdentity('profile'),name:profile.name+' '+t('team.copy')}])},t('team.copy')),
                        h('button',{type:'button',className:'dshTeamButton',onClick:()=>{const next=profiles.filter(p=>p.id!==profile.id);set({...value,profiles:next,defaultProfile:value.defaultProfile===profile.id?'':value.defaultProfile??'',defaultMode:value.defaultProfile===profile.id&&value.defaultMode==='custom'?'off':value.defaultMode??'off'});}},t('team.delete'))))),
                h('div',{className:'dshTeamToolbar'},h('button',{type:'button',className:'dshTeamButton',disabled:unavailable||profiles.length>=16,onClick:()=>field('profiles',[...profiles,{id:teamIdentity('profile'),name:t('team.newProfile'),roles:[blankRole()]}])},t('team.addProfile')),
                    h('button',{type:'button',className:'dshTeamButton',disabled:unavailable||!dirty.current,onClick:save},t('team.save')),
                    h('button',{type:'button',className:'dshTeamButton',disabled:unavailable,onClick:()=>{dirty.current=false;setDraft(structuredClone(snapshot?.value??{}));base.current=snapshot?.revision;setError(null);}},t('team.reset'))),
                renderDefaults&&h('details',{className:'dshTeamCard'},h('summary',null,t('team.advancedDefaults')),h('p',{className:'dshTeamHint'},t('team.advancedHint')),renderDefaults()),
                saved&&h('p',{role:'status',className:'dshTeamHint'},t('team.saved')),error&&h('p',{role:'alert',className:'dshTeamError'},error));
        }


		function apply(ctx) {
			ctx.effect(() => ctx.locale.register(NS, {
				zh,
				en
			}), "ui-subagent: dictionaries");
            const teamSettings=ctx.settingsScope.bind({namespace:"agent-teams"});
            const panel=createTeamPanel();
            const renderDefaults=()=>react.createElement(SubagentDefaultsSection,{api:ctx.get("connection").api});
            const rpc=async(method,payload)=>{const result=await ctx.get("connection").rpc.call("/api",method,payload);if(!result.ok)throw new Error(result.error?.message??"Request failed");return result.value;};
            const loadModels=teamModelDirectory;
            const saveSettings=async(value,expectedRevision)=>{await rpc("settings.mutate",{ns:"agent-teams",ops:[{op:"set",path:[],value}],expectedRevision});await teamSettings.load();};
            ctx.slots.inject("settings.section",()=>ctx.slots.register({name:"settings.section",id:"agent-teams",order:23,locale:NS,label:()=>ctx.locale.bind(NS)("team.title"),inject:()=>({scope:teamSettings,saveSettings,loadModels,renderDefaults})},TeamSettings));
			const sessions = ctx.sessions;
			const childLabels = (session, query) => {
				const { byId } = sessions.list.getSnapshot();
				return Object.values(byId).filter((child) => child.parentId === session.sessionId && child.running && child.displayTitle.includes(query)).map((child) => child.displayTitle);
			};
			const source = {
				trigger: "@",
				name: "subagent",
				candidates(session, { query }) {
					return Promise.resolve(childLabels(session, query).map((name) => ({ name })));
				},
				lexicon(session) {
					return childLabels(session, "");
				},
				subscribeLexicon(_session, listener) {
					return sessions.list.subscribe(listener);
				},
				onPick({ candidate }) {
					return { text: `@${candidate.name} ` };
				},
				codec: {
					clipboardText: (ref) => `@${ref}`,
					serialize: (ref) => Promise.resolve(`@${ref}`)
				}
			};
			const inputTriggers = ctx.get("inputTriggers");
			ctx.effect(() => inputTriggers.registerSource(source), "ui-subagent: @ source");
			const connection = ctx.get("connection");
			const loadProgress = async (address) => {
				const payload = { ...address, maxMessages: 8 };
				const response = connection.api?.subagents ? await connection.api.subagents.history(payload) : { result: await connection.rpc.call("/api", "subagent.history", payload) };
				if (!response.result.ok) throw new Error(response.result.error?.message ?? "Subtask history unavailable");
				return response.result.value;
			};
			const catalogActions = (parentSessionId) => ({
                parentSessionId,
                teamSettings,panel,loadModels,saveSettings,renderDefaults,refreshList:()=>sessions.refresh(),
                openMain: id=>sessions.open(id),
                async createMain(config){
                    const cwd=sessions.list.getSnapshot().byId[parentSessionId]?.cwd;
                    if(!cwd)throw new Error("Workspace unavailable");
                    const id=await sessions.create({cwd,sessionId:'agent-session-'+crypto.randomUUID()});
                    await rpc("session.rename",{sessionId:id,title:ctx.locale.bind(NS)("team.mainConversation")});
                    const created=await teamRequest(id);
                    await teamRequest(id,{action:'configure',mode:config.mode==='custom'?'custom':'auto',profileId:config.profile?.id??'',expectedRevision:created.board?.config?.revision??0});
                    await sessions.refresh();
                    sessions.open(id);
                },
                prepareTeam(text) {
                    const shell=ctx.get("conversation").input.shell(parentSessionId);
                    const existing=shell.snapshot.draft;
                    shell.setDraft(existing ? existing+"\n\n"+text : text);
                },
				sessionsStore: sessions.list,
				loadProgress,
				openChild(address) {
					sessions.openSubagent(address);
				},
				refresh(parentSessionId) {
					sessions.refreshSubagents(parentSessionId);
				},
				setCatalogOpen(parentSessionId, open) {
					sessions.setSubagentCatalogOpen(parentSessionId, open);
				}
			});
			ctx.slots.inject("tool.call.toolview", function* () {
				for (const key of ["subagent", "subagent_fork", "subagent_codex", "subagent_claude_code"]) yield ctx.slots.register({ name: "tool.call.toolview", key, locale: NS, inject: catalogActions }, SubagentToolRow);
			});

            ctx.slots.inject("conversation.session.header.actions", () => ctx.slots.register({ name: "conversation.session.header.actions", id: "agent-team-board", order: 11, locale: NS, inject: id=>({...catalogActions(id),header:true}) }, TeamComposerTrigger));
            ctx.slots.inject("conversation.collaboration.panel",()=>ctx.slots.register({name:"conversation.collaboration.panel",locale:NS,inject:id=>({...catalogActions(id),panelOnly:true})},TeamBoardAction));
            ctx.slots.inject("conversation.input.right",()=>ctx.slots.register({name:"conversation.input.right",id:"collaboration",order:90,locale:NS,inject:catalogActions},TeamComposerTrigger));
			ctx.slots.inject("conversation.composer", () => ctx.slots.register({
				name: "conversation.composer",
				priority: -10,
				locale: NS,
				select: selectReadOnlySubagent
			}, SubagentReadOnlyComposer));
		}
		//#endregion
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map
