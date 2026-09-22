window.__ModuleLoader__.load({
	id: "@deepseek-ai/dsh-client-ui-goal",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		let react_jsx_runtime = require("react/jsx-runtime");
		let react = require("react");
		let _deepseek_ai_dsh_client_ui_primitives = require("@deepseek-ai/dsh-client-ui-primitives");
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-goal\src\client\GoalBar.module.css.mjs
		const css$1 = ".NsqOXW_dock{box-sizing:border-box;width:calc(100% - var(--dsh-composer-side-clearance) - var(--dsh-composer-side-clearance) - var(--dsh-composer-dock-inset) - var(--dsh-composer-dock-inset) - var(--dsh-composer-dock-inset) - var(--dsh-composer-dock-inset));margin:0 auto}.NsqOXW_bar{box-sizing:border-box;width:100%;max-width:calc(var(--dsh-composer-card-max-width) - 4 * var(--dsh-composer-dock-inset));border:1px solid var(--dsw-alias-border-l1);background:var(--dsw-specific-tip);border-radius:12px;align-items:center;gap:10px;height:36px;margin:0 auto;padding:4px 5px 4px 12px;display:flex}.NsqOXW_goalGlyph{color:var(--dsw-alias-label-tertiary);flex:none;display:inline-flex}.NsqOXW_label{color:var(--dsw-alias-label-primary);flex:none;font-size:13px;font-weight:500;line-height:24px}.NsqOXW_objective{min-width:0;color:var(--dsw-alias-label-primary-dimmed);text-overflow:ellipsis;white-space:nowrap;flex:1;font-size:13px;line-height:20px;overflow:hidden}.NsqOXW_error{min-width:0;color:var(--dsw-alias-state-error-primary);text-overflow:ellipsis;white-space:nowrap;flex:1;font-size:12px;line-height:20px;overflow:hidden}.NsqOXW_objectiveInput{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-base);min-width:0;height:26px;color:var(--dsw-alias-label-primary);border-radius:6px;outline:none;flex:1;padding:0 8px;font-size:13px;line-height:20px}.NsqOXW_objectiveInput:focus{border-color:var(--dsw-alias-state-business-primary)}.NsqOXW_objectiveInput::placeholder{color:var(--dsw-alias-label-caption)}.NsqOXW_actions{flex:none;align-items:center;gap:10px;display:flex}.NsqOXW_iconBtn{width:28px;height:28px;color:var(--dsw-alias-label-tertiary);cursor:pointer;background:0 0;border:none;border-radius:999px;justify-content:center;align-items:center;padding:0;display:inline-flex}.NsqOXW_iconBtn:hover{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-secondary)}.NsqOXW_iconBtn:disabled{opacity:.4;cursor:default}";
		const tagId$1 = "@deepseek-ai/dsh-client-ui-goal/GoalBar.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId$1) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-goal";
			tag.dataset.pluginCss = tagId$1;
			tag.textContent = css$1 + ".dshGoalEdit{max-height:min(50dvh,420px);overflow:auto;box-sizing:border-box;display:grid;gap:10px;padding:12px;border:1px solid var(--dsw-alias-border-l2);border-radius:12px;background:var(--dsw-specific-tip);color:var(--dsw-alias-label-primary);font-size:13px;min-width:0}.dshGoalEdit textarea{box-sizing:border-box;width:100%;min-height:90px;max-height:240px;padding:8px;line-height:1.6;resize:vertical}.dshGoalEdit p{margin:0;white-space:pre-wrap;overflow-wrap:anywhere}.dshGoalEdit [role=alert]{color:var(--dsw-alias-state-error-primary)}.dshGoalEdit small,.dshGoalEdit [role=status]{color:var(--dsw-alias-label-secondary)}.dshGoalEditConflict{display:grid;gap:8px}.dshGoalEdit button:not(.NsqOXW_iconBtn){font:inherit;padding:6px 10px;color:inherit;background:var(--dsw-alias-bg-base);border:1px solid var(--dsw-alias-border-l2);border-radius:8px;cursor:pointer}.dshGoalDraftRow{display:flex;gap:10px;align-items:center;padding:6px 0}.dshGoalDraftRow span{flex:1;min-width:0;overflow-wrap:anywhere}";
			document.head.appendChild(tag);
		}
		var GoalBar_module_css_default = {
			"objective": "NsqOXW_objective",
			"bar": "NsqOXW_bar",
			"objectiveInput": "NsqOXW_objectiveInput",
			"dock": "NsqOXW_dock",
			"actions": "NsqOXW_actions",
			"goalGlyph": "NsqOXW_goalGlyph",
			"iconBtn": "NsqOXW_iconBtn",
			"label": "NsqOXW_label",
			"error": "NsqOXW_error"
		};
		//#endregion
		//#region lib/types/client/GoalBar.js
		/**
		* GoalBar: the goal indicator docked above the message composer (input dock
		* strip). A present goal shows a goal glyph, a phase label, the truncated
		* objective, and icon actions — resume when paused, edit (inline form in the
		* same strip), and clear. Goal creation lives on the `/goal` command, not
		* here: loading (undefined), no goal (null), and complete goals render
		* nothing. Live state arrives as the projected whole snapshot; the verbs are
		* the injected face.
		*/
		/** Strip label keys per visible phase; complete goals render nothing. */
		const PHASE_LABELS = {
			active: "phase.active",
			paused: "phase.paused",
			blocked: "phase.blocked"
		};
		const goalDrafts = require("@deepseek-ai/dsh-client-runtime/client").createRevisionDraftStore("goals");
		function GoalBar({ sessionId = "", goal, onEdit, onPause, onResume, onClear, t }) {
			const h = react.createElement, scopeKey = sessionId + ":" + (goal?.id ?? "");
			const scope = react.useRef({ key: scopeKey, epoch: 0 }), writer = react.useRef(null), alive = react.useRef(true), pendingRef = react.useRef(null);
			if (!writer.current) writer.current = globalThis.crypto.randomUUID();
			if (scope.current.key !== scopeKey) scope.current = { key: scopeKey, epoch: scope.current.epoch + 1 };
			const [editor, setEditor] = react.useState(null), [pending, setPending] = react.useState(false), [actionError, setActionError] = react.useState(null), [notice, setNotice] = react.useState(""), [records, setRecords] = react.useState([]), [clearedGoalId, setClearedGoalId] = react.useState(null);
			const latest = react.useRef(editor); latest.current = editor;
			react.useEffect(() => { alive.current = true; pendingRef.current = null; setPending(false); setEditor(null); setActionError(null); setNotice(""); setRecords([]); setClearedGoalId(null); return () => { alive.current = false; }; }, [scopeKey]);
			const current = token => alive.current && scope.current.key === token.key && scope.current.epoch === token.epoch;
			const ref = () => ({ id: goal.id, revision: goal.revision });
			const persist = value => {
				const result = goalDrafts.write(scopeKey, writer.current, value);
				setNotice(result.persisted ? t("draft.saved") : t("draft.storageError"));
				return result.persisted;
			};
			const change = value => { latest.current = value; setEditor(value); persist(value); setActionError(null); };
			const archive = () => { if (latest.current) goalDrafts.write(scopeKey, globalThis.crypto.randomUUID(), latest.current); };
			const beginEdit = () => {
				const saved = goalDrafts.list(scopeKey), restored = saved.find(row => row.value.data.ref?.revision === goal.revision);
				const next = restored?.value.data ?? { ref: ref(), draft: goal.objective };
				latest.current = next; setEditor(next); setRecords(saved); setActionError(null); setNotice(restored ? t("draft.restored") : "");
			};
			const runAction = async action => {
				if (pendingRef.current) return;
				const token = { ...scope.current }; pendingRef.current = token; setPending(true); setActionError(null);
				try { const result = await action(); if (!current(token)) return; if (!result.ok) setActionError(`${result.error.message} (${result.error.code})`); return result; }
				catch (reason) { if (!current(token)) return; const message = reason instanceof Error ? reason.message : String(reason); setActionError(message); return { ok: false }; }
				finally { if (current(token) && pendingRef.current === token) { pendingRef.current = null; setPending(false); } }
			};
			const conflict = !!editor && (editor.ref.id !== goal?.id || editor.ref.revision !== goal?.revision);
			const handleEdit = async () => {
				const actionScope = { ...scope.current };
				const draft = latest.current;
				if (!draft?.draft.trim() || conflict || pendingRef.current) return;
				const submitted = { ...draft, draft: draft.draft.trim() };
				if (!persist(submitted)) return;
				const result = await runAction(() => onEdit(submitted.draft, submitted.ref));
				if (result?.ok && current(actionScope)) {
					const durable = goalDrafts.complete(scopeKey, submitted, (a, b) => a.ref?.id === b.ref.id && a.ref?.revision === b.ref.revision && a.draft?.trim() === b.draft);
					setEditor(null); setNotice(durable ? "" : t("draft.receiptError"));
				}
			};
			const handleClear = async () => { const id = goal.id, expected = ref(), actionScope = { ...scope.current }; if ((await runAction(() => onClear(expected)))?.ok && current(actionScope)) setClearedGoalId(id); };
			if (!goal || goal.phase === "complete" || goal.id === clearedGoalId) return null;
			const iconButton = (label, Icon, action, disabled = pending) => h(_deepseek_ai_dsh_client_ui_primitives.Tooltip, { label: t(label), side: "bottom", delayMs: 500 }, h("button", { type: "button", className: GoalBar_module_css_default.iconBtn, "aria-label": t(label), disabled, onClick: action }, h(Icon, { size: 14 })));
			if (editor) return h("section", { className: GoalBar_module_css_default.dock, "data-goal-bar": true }, h("div", { className: "dshGoalEdit" },
				h("strong", null, t("action.edit")),
				h("textarea", { className: GoalBar_module_css_default.objectiveInput, "aria-label": t("objective.aria"), rows: 4, value: editor.draft, disabled: pending, autoFocus: true, onChange: event => change({ ...latest.current, draft: event.target.value }), onKeyDown: event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditor(null); } if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); handleEdit(); } } }),
				conflict && h("div", { className: "dshGoalEditConflict", role: "status" }, h("p", null, t("draft.conflict")), h("details", null, h("summary", null, t("draft.current")), h("p", null, goal.objective)), h("button", { type: "button", disabled: pending, onClick: () => { archive(); change({ ...latest.current, ref: ref() }); } }, t("draft.rebase")), h("button", { type: "button", disabled: pending, onClick: () => { archive(); change({ ref: ref(), draft: goal.objective }); } }, t("draft.useCurrent"))),
				records.length > 0 && h("details", null, h("summary", null, t("draft.other")), ...records.map(row => h("div", { key: row.key, className: "dshGoalDraftRow" }, h("span", null, row.value.data.draft?.slice(0, 100)), h("button", { type: "button", disabled: pending, onClick: () => { archive(); change(row.value.data); } }, t("draft.load"))))),
				actionError && h("p", { role: "alert" }, actionError), notice && h("p", { role: "status" }, notice), h("small", null, t("draft.keys")),
				h("div", { className: GoalBar_module_css_default.actions }, iconButton("action.save", _deepseek_ai_dsh_client_ui_primitives.IconCheckOutline16, handleEdit, pending || conflict || !editor.draft.trim()), iconButton("action.cancel", _deepseek_ai_dsh_client_ui_primitives.IconCloseOutline16, () => setEditor(null), false))));
			return h("div", { className: GoalBar_module_css_default.dock, "data-goal-bar": true }, h("div", { className: GoalBar_module_css_default.bar, title: goal.phase === "blocked" ? goal.blockedReason?.message : undefined },
				h("span", { className: GoalBar_module_css_default.goalGlyph }, h(_deepseek_ai_dsh_client_ui_primitives.IconGoalOutline16, { size: 14 })), h("span", { className: GoalBar_module_css_default.label }, t(PHASE_LABELS[goal.phase])), h("span", { className: GoalBar_module_css_default.objective }, goal.objective), actionError && h("span", { className: GoalBar_module_css_default.error, role: "alert", title: actionError }, actionError),
				h("div", { className: GoalBar_module_css_default.actions }, goal.phase === "active" && iconButton("action.pause", _deepseek_ai_dsh_client_ui_primitives.IconPauseOutline16, () => runAction(() => onPause(ref()))), ["paused", "blocked"].includes(goal.phase) && iconButton("action.resume", _deepseek_ai_dsh_client_ui_primitives.IconPlayOutline16, () => runAction(() => onResume(ref()))), iconButton("action.edit", _deepseek_ai_dsh_client_ui_primitives.IconEditOutline16, beginEdit), iconButton("action.clear", _deepseek_ai_dsh_client_ui_primitives.IconTrashOutline16, handleClear))));
		}
		/** Dock adapter: reads the host-computed 'goal' projection (whole value; absent or null renders nothing). */
		function GoalDock({ sessionId, useProjection, onEdit, onPause, onResume, onClear, t }) {
			const projection = useProjection("goal");
			return (0, react_jsx_runtime.jsx)(GoalBar, {
				goal: projection === void 0 ? void 0 : projection === null ? null : projection.goal,
				sessionId,
				onEdit,
				onPause,
				onResume,
				onClear,
				t
			});
		}
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-goal\src\client\GoalCommandInputView.module.css.mjs
		const css = "._9T-0mW_row{flex-direction:column;align-items:flex-end;gap:6px;display:flex}._9T-0mW_stack{flex-direction:column;align-items:flex-end;min-width:0;max-width:min(525px,82%);display:flex}._9T-0mW_bubble{overflow-wrap:anywhere;background:var(--dsw-specific-bubble);max-width:100%;color:var(--dsw-alias-label-primary);font:var(--dsw-font-markdown-code);white-space:pre-wrap;border-radius:22px;padding:10px 16px}";
		const tagId = "@deepseek-ai/dsh-client-ui-goal/GoalCommandInputView.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-goal";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var GoalCommandInputView_module_css_default = {
			"bubble": "_9T-0mW_bubble",
			"row": "_9T-0mW_row",
			"stack": "_9T-0mW_stack"
		};
		//#endregion
		//#region lib/types/client/GoalCommandInputView.js
		/** Right-aligned `/goal` input bubble without ordinary message actions. */
		const GoalCommandInputView = (0, react.memo)(function GoalCommandInputView({ node, t }) {
			const data = node.data;
			return (0, react_jsx_runtime.jsx)("div", {
				className: GoalCommandInputView_module_css_default.row,
				"data-command-input": "",
				role: "group",
				"aria-label": t("commandInput.aria"),
				children: (0, react_jsx_runtime.jsx)("div", {
					className: GoalCommandInputView_module_css_default.stack,
					children: (0, react_jsx_runtime.jsx)("div", {
						className: GoalCommandInputView_module_css_default.bubble,
						children: (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.MessageText, { text: data.text })
					})
				})
			});
		});
		//#endregion
		//#region lib/types/client/goal-command-input.js
		/**
		* Derive the visible command line from its structured durable run.
		* @param event - `/goal` command run.
		* @returns command text with trailing parser whitespace removed.
		*/
		function goalCommandText(event) {
			return `/${event.data.name}${(event.data.args ?? "").trimEnd()}`;
		}
		/** Goal-owned command input projection; the generic command Definition retains the result row. */
		const goalCommandInputDefinition = {
			kind: "goal-command-input",
			target: "chat",
			match: (event) => event.type === "command/run" && event.data.name === "goal" ? {
				id: String(event.data.commandId),
				role: "start"
			} : null,
			start: (_context, match) => {
				if (match.event.type !== "command/run") throw new Error("goal-command-input start requires command/run");
				return {
					commandId: match.event.data.commandId,
					seq: match.event.seq,
					time: match.event.time,
					text: goalCommandText(match.event)
				};
			},
			update: (context) => context.state,
			buildViewNode: (context) => {
				if (context.state === void 0) return null;
				return {
					key: context.key,
					kind: "command-input",
					id: context.id,
					target: "chat",
					anchorSeq: context.state.seq - .1,
					location: context.start?.location ?? { kind: "unresolved" },
					visibility: "visible",
					data: {
						commandId: context.state.commandId,
						text: context.state.text,
						time: context.state.time
					}
				};
			}
		};
		//#endregion
		//#region lib/types/client/locales.js
		/** `goal` namespace dictionaries. */
		/** Simplified Chinese dictionary (the key-set source of truth). */
		const zh = {
			"draft.saved": "草稿已保存在本机。",
			"draft.storageError": "本地存储不可用，草稿仅留在当前窗口；恢复存储后再保存目标。",
			"draft.receiptError": "目标已保存，本地草稿清理未完成。",
			"draft.restored": "已恢复未保存的草稿。",
			"draft.conflict": "目标已被其他操作更新；草稿已保留，请核对当前目标后再保存。",
			"draft.current": "查看当前目标",
			"draft.rebase": "保留草稿并使用当前版本",
			"draft.useCurrent": "使用当前目标内容",
			"draft.other": "可恢复的草稿",
			"draft.load": "载入草稿",
			"draft.keys": "Ctrl/⌘+Enter 保存；Esc 收起并保留草稿。",
			"phase.active": "进行中的目标",
			"phase.paused": "已暂停的目标",
			"phase.blocked": "受阻的目标",
			"objective.aria": "目标内容",
			"commandInput.aria": "命令输入",
			"action.save": "保存目标",
			"action.cancel": "收起并保留草稿",
			"action.pause": "暂停目标",
			"action.resume": "恢复目标",
			"action.edit": "编辑目标",
			"action.clear": "清除目标"
		};
		/** English dictionary, checked complete against the zh key set. */
		const en = {
			"draft.saved": "Draft saved on this device.",
			"draft.storageError": "Local storage is unavailable. Keep this window open and restore storage before saving the goal.",
			"draft.receiptError": "Goal saved; local draft cleanup is still pending.",
			"draft.restored": "Unsaved draft restored.",
			"draft.conflict": "The goal changed elsewhere. Your draft is preserved; review the current goal before saving.",
			"draft.current": "View current goal",
			"draft.rebase": "Keep draft against current revision",
			"draft.useCurrent": "Use current goal content",
			"draft.other": "Recoverable drafts",
			"draft.load": "Load draft",
			"draft.keys": "Ctrl/⌘+Enter saves; Esc closes and keeps the draft.",
			"phase.active": "Ongoing Goal",
			"phase.paused": "Paused Goal",
			"phase.blocked": "Blocked Goal",
			"objective.aria": "Goal objective",
			"commandInput.aria": "Command input",
			"action.save": "Save goal",
			"action.cancel": "Close and keep draft",
			"action.pause": "Pause goal",
			"action.resume": "Resume goal",
			"action.edit": "Edit goal",
			"action.clear": "Clear goal"
		};
		//#endregion
		//#region lib/types/client/index.js
		/** Dictionary namespace owned by this plugin. */
		const NS = "goal";
		/** Required services for the Goal dock, command-input projection, Remote mutations, and copy. */
		const inject = [
			"slots",
			"sessions",
			"remote",
			"remote.goals",
			"locale",
			"conversationEvents"
		];
		/**
		* Client plugin body: the GoalBar dock entry with its mutation verbs.
		* @param ctx - client root context.
		*/
		function apply(ctx) {
			ctx.conversationEvents.register(goalCommandInputDefinition);
			ctx.effect(() => ctx.locale.register(NS, {
				zh,
				en
			}), "ui-goal: dictionaries");
			ctx.slots.inject("conversation.chat.node", () => ctx.slots.register({
				name: "conversation.chat.node",
				key: "command-input",
				locale: NS
			}, GoalCommandInputView));
			const sessions = ctx.sessions;
			const noCurrentGoal = {
				ok: false,
				error: {
					code: "no-current-goal",
					message: "no current goal to mutate",
					details: {}
				}
			};
			ctx.slots.inject("conversation.input.dock", () => ctx.slots.register({
				name: "conversation.input.dock",
				id: "goal",
				order: 10,
				locale: NS,
				inject: (sessionId) => ({
					sessionId,
					onEdit: async (objective, expected) => {
						const ref = expected;
						if (ref === void 0) return noCurrentGoal;
						return await ctx.remote.goals.edit(sessionId, ref, { objective });
					},
					onPause: async (expected) => {
						const ref = expected;
						if (ref === void 0) return noCurrentGoal;
						return await ctx.remote.goals.pause(sessionId, ref);
					},
					onResume: async (expected) => {
						const ref = expected;
						if (ref === void 0) return noCurrentGoal;
						return await ctx.remote.goals.resume(sessionId, ref);
					},
					onClear: async (expected) => {
						const ref = expected;
						if (ref === void 0) return noCurrentGoal;
						return await ctx.remote.goals.clear(sessionId, ref);
					}
				})
			}, GoalDock));
		}
		//#endregion
		exports.GoalBar = GoalBar;
		exports.GoalDock = GoalDock;
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map
