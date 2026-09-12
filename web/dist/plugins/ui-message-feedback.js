window.__ModuleLoader__.load({
	id: "@deepseek-ai/dsh-client-ui-message-feedback",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		let react_jsx_runtime = require("react/jsx-runtime");
		let react = require("react");
		let _deepseek_ai_dsh_client_ui_primitives = require("@deepseek-ai/dsh-client-ui-primitives");
		//#region lib/types/client/controller.js
		const INITIAL_VIEW = Object.freeze({
			status: "cold",
			items: /* @__PURE__ */ new Map(),
			error: null
		});
		const OK = Object.freeze({ ok: true });
		const DISPOSED = Object.freeze({
			ok: false,
			error: Object.freeze({
				code: "disposed",
				message: "feedback controller is disposed"
			})
		});
		/** Human-readable text for one business failure code. */
		function describe(code) {
			switch (code) {
				case "session-not-found": return "this session is no longer persisted";
				case "target-not-found": return "this message is not a persisted assistant message";
				case "version-conflict": return "feedback changed elsewhere";
				case "note-blank": return "a note must contain a non-whitespace character";
				case "note-too-large": return "the note is too long";
				default: return code;
			}
		}
		/** Build the rejected branch for one business failure code. */
		function fail(code) {
			return {
				ok: false,
				error: {
					code,
					message: describe(code)
				}
			};
		}
		/** Carrier failure rendered with the Host-supplied code and message. */
		function carrierFailure(error) {
			return {
				ok: false,
				error: {
					code: error.code,
					message: error.message
				}
			};
		}
		/**
		* Per-session feedback object layer. One instance backs every per-message
		* control in that Session, so a single list read seeds them all.
		*/
		var MessageFeedbackController = class {
			remote;
			sessionId;
			view = INITIAL_VIEW;
			listeners = /* @__PURE__ */ new Set();
			loadPromise = null;
			operationTail = Promise.resolve();
			disposed = false;
			/**
			* @param remote - the messageFeedback Remote namespace.
			* @param sessionId - Session owning every addressed assistant message.
			*/
			constructor(remote, sessionId) {
				this.remote = remote;
				this.sessionId = sessionId;
			}
			/** Return the cached immutable view. */
			getSnapshot = () => this.view;
			/** Subscribe to view replacement. */
			subscribe = (listener) => {
				this.listeners.add(listener);
				return () => {
					this.listeners.delete(listener);
				};
			};
			/**
			* Load once; a failed load stays retryable.
			* @returns the settled load result, shared by concurrent callers.
			*/
			ensure() {
				if (this.view.status === "ready") return Promise.resolve(OK);
				return this.refresh();
			}
			/**
			* Re-read the authoritative list, collapsing concurrent callers onto one
			* in-flight read.
			*
			* This is the unserialized read used to seed a cold controller, where no
			* mutation can be in flight yet. A reconnect must use {@link resync} instead:
			* an unserialized list response can otherwise arrive after a newer mutation's
			* reply and overwrite the version that mutation just committed.
			* @returns the settled reload result.
			*/
			refresh() {
				if (this.loadPromise !== null) return this.loadPromise;
				this.publish({
					status: "loading",
					items: this.view.items,
					error: null
				});
				const pending = this.load();
				this.loadPromise = pending;
				return pending.finally(() => {
					this.loadPromise = null;
				});
			}
			/**
			* Re-read the list behind this Session's queued mutations, so a reconnect
			* cannot resurrect a version an in-flight mutation already replaced.
			* @returns the settled reload result.
			*/
			resync() {
				return this.mutate(() => this.refresh(), { seed: false });
			}
			/**
			* Create or replace feedback for one message, comparing against the version
			* this controller last observed.
			*
			* The note is resolved here rather than by the caller: `mutate` awaits the
			* one list read first, so this body always sees the committed item, while a
			* control that rendered before that read completed would still be holding
			* `undefined`. Omitting `note` therefore keeps whatever is stored; only
			* {@link clearNote} removes one.
			* @param messageId - target assistant message.
			* @param rating - desired judgment.
			* @param note - replacement explanation; omitted keeps the stored note.
			* @returns the settled mutation result.
			*/
			rate(messageId, rating, note) {
				return this.mutate(async () => {
					const observed = this.view.items.get(messageId);
					return await this.putCommitted(messageId, rating, note ?? observed?.note, observed);
				});
			}
			/**
			* Replace one message's rating with the opposite judgment, or retract it when
			* the committed rating already matches. The decision reads the committed item
			* inside the serialized mutation, so a click that lands before the first list
			* read still toggles against the stored value rather than the empty view a
			* cold control rendered.
			* @param messageId - target assistant message.
			* @param rating - the judgment the human asked for.
			* @returns the settled mutation result.
			*/
            retract(messageId, rating) {
                return this.mutate(async () => {
                    const observed = this.view.items.get(messageId);
                    return observed?.rating === rating ? await this.deleteCommitted(messageId, observed) : OK;
                });
            }
            confirmRating(messageId, rating, note, category) {
                return this.mutate(async () => this.putCommitted(messageId, rating, note, this.view.items.get(messageId), category ?? null));
            }
			toggle(messageId, rating) {
				return this.mutate(async () => {
					const observed = this.view.items.get(messageId);
					if (observed?.rating === rating) return await this.deleteCommitted(messageId, observed);
					return await this.putCommitted(messageId, rating, observed?.note, observed);
				});
			}
			/**
			* Drop the note while keeping the rating. Absent feedback needs no call.
			* @param messageId - target assistant message.
			* @returns the settled mutation result.
			*/
			clearNote(messageId) {
				return this.mutate(async () => {
					const observed = this.view.items.get(messageId);
					if (observed === void 0 || observed.note === void 0) return OK;
					return await this.putCommitted(messageId, observed.rating, void 0, observed);
				});
			}
			/**
			* Remove feedback for one message. A message with no known item is already
			* in the requested state, so no call is made.
			* @param messageId - target assistant message.
			* @returns the settled mutation result.
			*/
			clear(messageId) {
				return this.mutate(async () => {
					const observed = this.view.items.get(messageId);
					if (observed === void 0) return OK;
					return await this.deleteCommitted(messageId, observed);
				});
			}
			/** Commit one put against the observed version and reconcile a conflict. */
			async putCommitted(messageId, rating, note, observed, category = observed?.category) {
				const carried = await this.remote.put({
					sessionId: this.sessionId,
					messageId,
					rating,
					...note === void 0 ? {} : { note },
                    ...category == null ? {} : { category },
					ifVersion: observed?.version ?? null
				});
				if (!carried.ok) return carrierFailure(carried.error);
				const result = carried.value;
				if (result.ok) {
					this.commit(messageId, result.value);
					return OK;
				}
				if (result.error.code === "version-conflict") this.commit(messageId, result.error.current);
				return fail(result.error.code);
			}
			/** Commit one delete against the observed version and reconcile a conflict. */
			async deleteCommitted(messageId, observed) {
				const carried = await this.remote.delete({
					sessionId: this.sessionId,
					messageId,
					ifVersion: observed.version
				});
				if (!carried.ok) return carrierFailure(carried.error);
				const result = carried.value;
				if (result.ok) {
					this.commit(messageId, null);
					return OK;
				}
				if (result.error.code === "version-conflict") this.commit(messageId, result.error.current);
				return fail(result.error.code);
			}
			/** Drop subscribers and refuse further work when the owning fiber unloads. */
			dispose() {
				this.disposed = true;
				this.listeners.clear();
			}
			/** Fetch the whole sidecar and publish it as the seeded view. */
			async load() {
				try {
					const carried = await this.remote.list({ sessionId: this.sessionId });
					if (this.disposed) return OK;
					if (!carried.ok) {
						this.publish({
							status: "error",
							items: this.view.items,
							error: carried.error.message
						});
						return carrierFailure(carried.error);
					}
					const result = carried.value;
					if (!result.ok) {
						this.publish({
							status: "error",
							items: this.view.items,
							error: describe(result.error.code)
						});
						return fail(result.error.code);
					}
					const items = /* @__PURE__ */ new Map();
					for (const item of result.value.items) items.set(item.messageId, item);
					this.publish({
						status: "ready",
						items,
						error: null
					});
					return OK;
				} catch (error) {
					if (this.disposed) return OK;
					const message = error instanceof Error ? error.message : "message feedback list failed";
					this.publish({
						status: "error",
						items: this.view.items,
						error: message
					});
					return {
						ok: false,
						error: {
							code: "transport",
							message
						}
					};
				}
			}
			/**
			* Serialize one mutation behind this Session's prior mutation so queued
			* operations always compare against the committed version, and translate a
			* transport throw into the same settled shape the controls already render.
			*/
			mutate(operation, options = {}) {
				const guarded = async () => {
					if (this.disposed) return DISPOSED;
					if (options.seed !== false) {
						const loaded = await this.ensure();
						if (!loaded.ok) return loaded;
						if (this.disposed) return DISPOSED;
					}
					try {
						return await operation();
					} catch (error) {
						return {
							ok: false,
							error: {
								code: "transport",
								message: error instanceof Error ? error.message : "message feedback mutation failed"
							}
						};
					}
				};
				const result = this.operationTail.then(guarded, guarded);
				this.operationTail = result.then(() => void 0);
				return result;
			}
			/**
			* Replace one message's entry, keeping every other entry's identity. Only a
			* `mutate` operation reaches this, and `mutate` refuses admission once the
			* controller is disposed, so no disposal guard belongs here; `publish` is
			* the single place that stops notifying after listeners are dropped.
			*/
			commit(messageId, item) {
				const items = new Map(this.view.items);
				if (item === null) items.delete(messageId);
				else items.set(messageId, item);
				this.publish({
					status: "ready",
					items,
					error: null
				});
			}
			/** Replace the view and contain subscriber failures at the observable boundary. */
			publish(view) {
				this.view = Object.freeze(view);
				for (const listener of this.listeners) try {
					listener();
				} catch (error) {
					console.error("[ui-message-feedback] subscriber threw:", error);
				}
			}
		};
		//#endregion
		//#region \0dsh-css:D:\HermesTemp\deepseek-harness\packages\client\ui-message-feedback\src\client\MessageFeedbackActions.module.css.mjs
		const css = ".eTUJsW_action{width:28px;height:28px;color:var(--dsw-alias-label-tertiary);cursor:pointer;background:0 0;border:none;border-radius:28px;justify-content:center;align-items:center;padding:6px;display:inline-flex}.eTUJsW_action:hover{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-secondary)}.eTUJsW_action:disabled{cursor:default;opacity:.4}.eTUJsW_action[data-active]{color:var(--dsw-alias-label-primary)}.eTUJsW_noteOpen{max-width:220px;color:var(--dsw-alias-label-tertiary);white-space:nowrap;text-overflow:ellipsis;cursor:pointer;background:0 0;border:none;border-radius:14px;padding:0 8px;font-size:13px;line-height:28px;overflow:hidden}.eTUJsW_noteOpen:hover{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-secondary)}.eTUJsW_noteEditor{align-items:flex-start;gap:6px;display:inline-flex}.eTUJsW_noteInput{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-base);width:260px;color:var(--dsw-alias-label-primary);font:inherit;resize:vertical;border-radius:8px;padding:6px 8px;font-size:13px}.eTUJsW_noteSave,.eTUJsW_noteCancel{cursor:pointer;border:none;border-radius:14px;height:28px;padding:0 10px;font-size:13px}.eTUJsW_noteSave{background:var(--dsw-alias-interactive-bg-primary);color:var(--dsw-alias-label-inverse)}.eTUJsW_noteSave:disabled{cursor:default;opacity:.4}.eTUJsW_noteCancel{color:var(--dsw-alias-label-tertiary);background:0 0}.eTUJsW_noteCancel:hover{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-secondary)}.eTUJsW_failure{color:var(--dsw-alias-label-tertiary);padding-left:4px;font-size:13px;line-height:28px}";
		const tagId = "@deepseek-ai/dsh-client-ui-message-feedback/MessageFeedbackActions.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "@deepseek-ai/dsh-client-ui-message-feedback";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var MessageFeedbackActions_module_css_default = {
			"noteInput": "eTUJsW_noteInput",
			"noteSave": "eTUJsW_noteSave",
			"noteEditor": "eTUJsW_noteEditor",
			"action": "eTUJsW_action",
			"noteCancel": "eTUJsW_noteCancel",
			"noteOpen": "eTUJsW_noteOpen",
			"failure": "eTUJsW_failure"
		};
		//#endregion
		//#region lib/types/client/MessageFeedbackActions.js
		/**
		* Per-message feedback controls: a Like/Dislike pair plus an optional note.
		* Rendered inside the assistant message's IconActions row, so the buttons
		* reuse that row's chrome and sit between copy and branch.
		* @module @deepseek-ai/dsh-client-ui-message-feedback/client/MessageFeedbackActions
		*/
		/**
		* One message's feedback controls.
		* @param props - the owner's message identity, the injected verbs, and the
		* shared feedback hook.
		* @returns the rating buttons, plus the note editor while it is open.
		*/
		function FeedbackDeliverySettings({ scope, controls, t }) {
			const snapshot=(0,react.useSyncExternalStore)(listener=>scope.subscribe(listener),()=>scope.getSnapshot(),()=>scope.getSnapshot());
			const [endpoint,setEndpoint]=(0,react.useState)(""),[busy,setBusy]=(0,react.useState)(false),[error,setError]=(0,react.useState)("");
			const configured=snapshot.secrets?.some(secret=>secret.path.length===1&&secret.path[0]==="endpoint"&&secret.set)===true;
			const disabled=busy||snapshot.status!=="ready"||!snapshot.writable;
			const save=async(key,value)=>{if(disabled)return;setBusy(true);setError("");try{await scope.setChecked(key,value);if(key==="endpoint")setEndpoint("")}catch(failure){setError(failure.message||String(failure))}finally{setBusy(false)}};
			return (0,react_jsx_runtime.jsxs)("section",{className:"dswSuiteSettings",children:[
				(0,react_jsx_runtime.jsx)("p",{children:t("delivery.description")}),
				(0,react_jsx_runtime.jsxs)("div",{className:"dswSuiteSetting",children:[(0,react_jsx_runtime.jsx)("span",{children:t("delivery.enabled")}), (0,react_jsx_runtime.jsx)(controls.Switch,{checked:snapshot.value?.enabled===true,disabled,label:t("delivery.enabled"),onChange:value=>save("enabled",value)})]}),
				(0,react_jsx_runtime.jsx)(controls.SecretField,{id:"feedback-recipient",label:t("delivery.endpoint"),configured,stateLabel:t(configured?"delivery.configured":"delivery.unconfigured"),text:endpoint,disabled,onEdit:setEndpoint,hint:t("delivery.endpointHint")}),
				(0,react_jsx_runtime.jsxs)("div",{className:"dswSuiteToolbar",children:[(0,react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button,{variant:"outline",size:"sm",disabled:disabled||!endpoint.trim(),onClick:()=>save("endpoint",endpoint.trim()),children:t("note.save")}), (0,react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button,{variant:"ghost",size:"sm",disabled:disabled||!configured,onClick:()=>save("endpoint",""),children:t("delivery.clear")})]}),
				(snapshot.status!=="ready"||error)&&(0,react_jsx_runtime.jsx)("p",{role:"status",children:error||snapshot.error||t(snapshot.status==="unavailable"?"delivery.readonly":"delivery.loading")})
			]});
		}
		async function feedbackSubmissionRequest(action, body) {
			const response = await fetch(`/__dsh-feedback/${action}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
			const value = await response.json();
			if (!response.ok) throw new Error(value.message ?? value.error ?? "反馈提交失败");
			return value;
		}
		function feedbackSubmissionId() {
			return globalThis.crypto?.randomUUID?.() ?? "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, value => { const random = Math.random() * 16 | 0; return (value === "x" ? random : random & 3 | 8).toString(16); });
		}
		const SESSION_FEEDBACK_CATEGORIES = ["task-result","instruction-following","product-interaction","service-stability","resource-cost","security-privacy-permission","other"];
		async function sessionFeedbackRequest(payload) {
			const rpcId = feedbackSubmissionId();
			const response = await fetch("/api/sessionFeedback.record", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({type:"client-request",rpcId,method:"sessionFeedback.record",payload})});
			if (!response.ok) throw new Error(`Session feedback HTTP ${response.status}`);
			const envelope = await response.json();
			if (envelope.rpcId !== rpcId) throw new Error("Session feedback response id mismatch");
			if (!envelope.result?.ok) throw new Error(envelope.result?.error?.message ?? "Could not record feedback");
			if (!envelope.result.value?.ok) throw new Error(envelope.result.value?.error?.message ?? "Could not record feedback");
			if (envelope.result.value.value?.recorded !== true) throw new Error("Feedback was not acknowledged");
			return envelope.result.value.value;
		}
		function SessionFeedbackEntry({sessionId,t}) {
			const h=react.createElement;
			const [open,setOpen]=react.useState(false),[text,setText]=react.useState(""),[category,setCategory]=react.useState(""),[busy,setBusy]=react.useState(false),[error,setError]=react.useState(""),[saved,setSaved]=react.useState(false);
			const requestId=react.useRef(feedbackSubmissionId()),running=react.useRef(false),alive=react.useRef(true);
			react.useEffect(()=>{alive.current=true;return()=>{alive.current=false;};},[]);
			const change=(setter,value)=>{setter(value);requestId.current=feedbackSubmissionId();setSaved(false);setError("");};
			const save=async()=>{
				if(running.current||(!text.trim()&&!category))return;
				running.current=true;setBusy(true);setError("");
				try { await sessionFeedbackRequest({sessionId,requestId:requestId.current,...(text.trim()?{text:text.trim()}:{}),...(category?{category}:{})});
					if(alive.current){setSaved(true);setOpen(false);setText("");setCategory("");requestId.current=feedbackSubmissionId();}
				} catch(failure) {if(alive.current)setError(failure.message);}
				finally {running.current=false;if(alive.current)setBusy(false);}
			};
			return h(react.Fragment,null,
				h(_deepseek_ai_dsh_client_ui_primitives.Button,{variant:"ghost",size:"sm",onClick:()=>{setOpen(true);setSaved(false);}},t("session.title")),
				saved&&h("span",{role:"status",style:{fontSize:12,color:"var(--dsw-alias-label-tertiary)"}},t("session.recorded")),
				open&&h(_deepseek_ai_dsh_client_ui_primitives.Modal,{open,title:t("session.title"),onClose:()=>{if(!busy)setOpen(false);},closeLabel:t("note.cancel"),footer:h(react.Fragment,null,
					h(_deepseek_ai_dsh_client_ui_primitives.Button,{variant:"outline",disabled:busy,onClick:()=>setOpen(false)},t("note.cancel")),
					h(_deepseek_ai_dsh_client_ui_primitives.Button,{variant:"primary",disabled:busy||(!text.trim()&&!category),onClick:save},busy?t("session.saving"):t("note.save")))},
					h("p",null,t("session.description")),
					h("label",{style:{display:"grid",gap:8,marginBottom:16}},t("session.category"),h("select",{value:category,"aria-label":t("session.category"),disabled:busy,onChange:event=>change(setCategory,event.target.value),style:{font:"inherit",color:"inherit",background:"var(--dsw-alias-bg-layer-1)",border:"1px solid var(--dsw-alias-border-l2)",borderRadius:8,padding:8}},h("option",{value:""},t("session.choose")),...SESSION_FEEDBACK_CATEGORIES.map(value=>h("option",{key:value,value},t(`session.category.${value}`))))),
					h("textarea",{className:MessageFeedbackActions_module_css_default.noteInput,style:{width:"100%",boxSizing:"border-box"},rows:4,maxLength:20000,value:text,disabled:busy,"aria-label":t("note.aria"),placeholder:t("session.placeholder"),onChange:event=>change(setText,event.target.value)}),
					error&&h("p",{role:"alert",style:{color:"var(--dsw-alias-state-error-primary)"}},error)));
		}
		function FeedbackSubmissionDialog({ sessionId, messageId, open, onClose, t }) {
			const key = `dsh.feedback.submission:${sessionId}:${messageId}`;
			const [submissionId, setSubmissionId] = (0, react.useState)(() => { try { return localStorage.getItem(key) || feedbackSubmissionId(); } catch { return feedbackSubmissionId(); } });
			const [packet, setPacket] = (0, react.useState)(null), [busy, setBusy] = (0, react.useState)(false), [error, setError] = (0, react.useState)("");
			const alive = (0, react.useRef)(true);
			(0, react.useEffect)(() => { alive.current = true; return () => { alive.current = false; }; }, []);
			(0, react.useEffect)(() => {
				if (!open) return;
				let active = true; setBusy(true); setError(""); setPacket(null);
				try { localStorage.setItem(key, submissionId); } catch {}
				feedbackSubmissionRequest("prepare", { sessionId, messageId, requestId: submissionId }).then(value => { if (active) setPacket(value); }, failure => { if (active) setError(failure.message); }).finally(() => { if (active) setBusy(false); });
				return () => { active = false; };
			}, [open, sessionId, messageId, submissionId]);
			const send = async () => {
				if (busy || !packet?.canSend) return;
				setBusy(true); setError("");
				try { const value = await feedbackSubmissionRequest("send", { sessionId, submissionId, destinationKey: packet.destinationKey }); if (alive.current) setPacket(value); }
				catch (failure) { if (alive.current) setError(failure.message); try { const value = await feedbackSubmissionRequest("status", { sessionId, submissionId }); if (alive.current) setPacket(value); } catch {} }
				finally { if (alive.current) setBusy(false); }
			};
			const download = () => { if (!packet) return; const url = URL.createObjectURL(new Blob([JSON.stringify(packet.payload, null, 2)], { type: "application/json" })); const link = document.createElement("a"); link.href = url; link.download = `feedback-${submissionId}.json`; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000); };
			return (0, react_jsx_runtime.jsxs)(_deepseek_ai_dsh_client_ui_primitives.Modal, {
				open, onClose, title: t("submission.title"), closeLabel: t("note.cancel"),
				footer: (0, react_jsx_runtime.jsxs)(react_jsx_runtime.Fragment, { children: [
					(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "outline", disabled: busy, onClick: () => setSubmissionId(feedbackSubmissionId()), children: t("submission.refresh") }),
					(0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "outline", disabled: !packet || busy, onClick: download, children: t("submission.download") }),
					packet?.canSend && (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "primary", disabled: busy, onClick: send, children: busy ? t("submission.sending") : packet.status === "failed" ? t("submission.retry") : t("submission.send") })
				] }),
				children: [
					(0, react_jsx_runtime.jsx)("p", { children: t("submission.description") }),
					packet && (0, react_jsx_runtime.jsxs)("p", { role: "status", children: [t(`submission.status.${packet.status}`), packet.destination ? ` · ${packet.destination}` : ` · ${t("submission.localOnly")}`] }),
					packet && (0, react_jsx_runtime.jsx)("p", { children: t("submission.summary", { count: packet.payload.messages.length, seq: packet.payload.capturedThroughSeq ?? "—", omitted: packet.payload.omittedMessages ?? 0 }) }),
					busy && !packet && (0, react_jsx_runtime.jsx)("p", { role: "status", children: t("submission.preparing") }),
					packet && (0, react_jsx_runtime.jsxs)("details", { className: "dshSettingsDisclosure", children: [(0, react_jsx_runtime.jsx)("summary", { children: t("submission.preview") }), (0, react_jsx_runtime.jsx)("pre", { style: { maxHeight: 280, overflow: "auto", whiteSpace: "pre-wrap", fontSize: 12, padding: 12, background: "var(--dsw-alias-markdown-code-block)", borderRadius: 8 }, children: JSON.stringify(packet.payload, null, 2) })] }),
					(error || packet?.lastError) && (0, react_jsx_runtime.jsx)("p", { role: "alert", style: { color: "var(--dsw-alias-state-error-primary)" }, children: error || packet.lastError })
				]
			});
		}
        function RatingConfirmation({ draft, submit, onClose, t }) {
            const h = react.createElement;
            const [note, setNote] = react.useState(draft.note ?? "");
            const [category, setCategory] = react.useState(draft.category ?? "");
            const [busy, setBusy] = react.useState(false), [error, setError] = react.useState(null);
            const saving = react.useRef(false), alive = react.useRef(true);
            react.useEffect(() => { alive.current = true; return () => { alive.current = false; if (draft.returnFocus?.isConnected && document.activeElement === document.body) draft.returnFocus.focus(); }; }, []);
            const save = async () => {
                if (saving.current) return;
                saving.current = true; setBusy(true); setError(null);
                try {
                    const result = await submit(draft.rating, note.trim() || undefined, category || undefined);
                    if (!alive.current) return;
                    if (result.ok) onClose(true);
                    else setError(result.error?.code === "version-conflict" ? t("error.conflict") : result.error?.code === "note-too-large" ? t("confirm.tooLong") : t("error.generic"));
                } catch { if (alive.current) setError(t("error.generic")); }
                finally { saving.current = false; if (alive.current) setBusy(false); }
            };
            return h(_deepseek_ai_dsh_client_ui_primitives.Modal, {
                open: true, title: t(draft.rating === "positive" ? "confirm.positive" : "confirm.negative"),
                closeLabel: t("note.cancel"), onClose: () => onClose(false),
                footer: h(react.Fragment, null,
                    h(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "outline", onClick: () => onClose(false) }, t("note.cancel")),
                    h(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "primary", disabled: busy, onClick: save }, busy ? t("session.saving") : t("confirm.save")))
            },
                h("p", null, t("confirm.description")),
                h("label", { style: { display: "grid", gap: 8, marginBottom: 16 } }, t("session.category"),
                    h("select", { value: category, disabled: busy, "aria-label": t("session.category"), onChange: event => setCategory(event.target.value), style: { font: "inherit", color: "inherit", background: "var(--dsw-alias-bg-layer-1)", border: "1px solid var(--dsw-alias-border-l2)", borderRadius: 8, padding: 8 } },
                        h("option", { value: "" }, t("session.choose")),
                        ...SESSION_FEEDBACK_CATEGORIES.map(value => h("option", { key: value, value }, t(`session.category.${value}`))))),
                h("textarea", { className: MessageFeedbackActions_module_css_default.noteInput, style: { width: "100%", boxSizing: "border-box" }, rows: 4, autoFocus: true, value: note, disabled: busy, "aria-label": t("note.aria"), placeholder: t("note.placeholder"), onChange: event => setNote(event.target.value) }),
                error && h("div", { role: "alert", style: { color: "var(--dsw-alias-state-error-primary)" } }, error,
                    h(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "ghost", onClick: () => setError(null), "aria-label": t("confirm.dismissError") }, t("confirm.dismissError"))));
        }
        function MessageFeedbackActions({ sessionId, messageId, ensure, readItem, confirmRating, retract, useFeedback, t }) {
            const h = react.createElement;
            const item = useFeedback(view => view.items.get(messageId));
            const loadFailed = useFeedback(view => view.status === "error");
            const [dialog, setDialog] = react.useState(null), [submissionOpen, setSubmissionOpen] = react.useState(false);
            const [pending, setPending] = react.useState(false), [failure, setFailure] = react.useState(null), [notice, setNotice] = react.useState(null);
            const operation = react.useRef(null), nextId = react.useRef(0), alive = react.useRef(true);
            const key = `${sessionId}:${messageId}`, currentKey = react.useRef(key); currentKey.current = key;
            react.useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
            react.useEffect(() => { operation.current = null; setPending(false); setDialog(null); setSubmissionOpen(false); setFailure(null); setNotice(null); void ensure(); }, [sessionId, messageId]);
            const seed = () => { void ensure(); };
            const choose = async (rating, editing = false) => {
                if (operation.current !== null) return;
                const token = { key, id: ++nextId.current, returnFocus: document.activeElement }; operation.current = token;
                const valid = () => alive.current && currentKey.current === key && operation.current === token;
                setPending(true); setFailure(null); setNotice(null);
                try {
                    const loaded = await ensure();
                    if (!valid()) return;
                    if (!loaded.ok) { setFailure(t("error.load")); return; }
                    const observed = readItem(messageId);
                    if (editing) {
                        if (observed) setDialog({ id: token.id, rating: observed.rating, note: observed.note, category: observed.category, returnFocus: token.returnFocus });
                    } else if (observed?.rating === rating) {
                        const result = await retract(messageId, rating);
                        if (valid()) {
                            if (result.ok) setNotice(t("confirm.updated"));
                            else setFailure(result.error?.code === "version-conflict" ? t("error.conflict") : t("error.generic"));
                        }
                    } else setDialog({ id: token.id, rating, note: "", category: "", returnFocus: token.returnFocus });
                } catch { if (valid()) setFailure(t("error.generic")); }
                finally { if (valid()) { operation.current = null; setPending(false); } }
            };
            const rateButton = rating => {
                const active = item?.rating === rating;
                const label = t(rating === "positive" ? active ? "action.likeActive" : "action.like" : active ? "action.dislikeActive" : "action.dislike");
                return h(_deepseek_ai_dsh_client_ui_primitives.Tooltip, { label, side: "bottom" },
                    h("button", { type: "button", className: MessageFeedbackActions_module_css_default.action, "aria-label": label, "aria-pressed": active, "data-active": active || undefined, disabled: pending, onFocus: seed, onPointerEnter: seed, onClick: () => choose(rating) },
                        h(rating === "positive" ? _deepseek_ai_dsh_client_ui_primitives.IconLikeOutline16 : _deepseek_ai_dsh_client_ui_primitives.IconDislikeOutline16)));
            };
            return h(react.Fragment, null, rateButton("positive"), rateButton("negative"),
                dialog && h(RatingConfirmation, { key: `${key}:${dialog.id}`, draft: dialog, t,
                    submit: (rating, note, category) => confirmRating(messageId, rating, note, category),
                    onClose: saved => { setDialog(current => current?.id === dialog.id ? null : current); if (saved && currentKey.current === key) setNotice(t("confirm.saved")); } }),
                item && h("button", { type: "button", className: MessageFeedbackActions_module_css_default.noteOpen, disabled: pending, onClick: () => choose(item.rating, true) }, item.note ?? t("note.open")),
                item && h(_deepseek_ai_dsh_client_ui_primitives.Button, { variant: "ghost", size: "sm", onClick: () => setSubmissionOpen(true) }, t("submission.title")),
                submissionOpen && h(FeedbackSubmissionDialog, { key, sessionId, messageId, open: true, onClose: () => setSubmissionOpen(false), t }),
                (failure || loadFailed) && h("span", { role: "alert", className: MessageFeedbackActions_module_css_default.failure }, failure ?? t("error.load")),
                notice && h("span", { role: "status", style: { fontSize: 12, color: "var(--dsw-alias-label-tertiary)" } }, notice));
        }
		//#endregion
		//#region lib/types/client/locales.js
		/** `feedback` namespace dictionaries. */
		/** Simplified Chinese dictionary (the key-set source of truth). */
		const zh = {
            "confirm.positive": "确认正面评价",
            "confirm.negative": "确认负面评价",
            "confirm.save": "确认评价",
            "confirm.description": "记录对这条回复的评价。分类和说明可选；向接收端发送需要另行确认。",
            "confirm.saved": "评价已记录",
            "confirm.updated": "评价状态已更新",
            "confirm.tooLong": "评价说明过长，请缩短后重试",
            "confirm.dismissError": "关闭错误提示",

			"session.title":"会话反馈","session.recorded":"反馈已记录","session.saving":"正在保存…","session.description":"记录对整个会话的意见，不会启动模型请求；会话共享仍由当前共享设置控制。","session.category":"反馈分类","session.choose":"选择分类（可选）","session.placeholder":"描述具体问题或建议（可选）",
			"session.category.task-result":"任务结果","session.category.instruction-following":"指令遵循","session.category.product-interaction":"交互体验","session.category.service-stability":"服务稳定性","session.category.resource-cost":"资源与费用","session.category.security-privacy-permission":"安全、隐私与权限","session.category.other":"其他",
			"delivery.title": "反馈交付",
			"delivery.description": "反馈默认保存在本地。启用接收端后，仍需在提交包中查看内容并点击发送。",
			"delivery.enabled": "允许提交到接收端",
			"delivery.endpoint": "接收地址",
			"delivery.configured": "已配置",
			"delivery.unconfigured": "未配置",
			"delivery.endpointHint": "使用 HTTPS 接收地址；本机调试可用 HTTP。地址只写入配置，不会回显；留空保留现有地址。",
			"delivery.clear": "清除地址",
			"delivery.loading": "正在读取反馈设置…",
			"delivery.readonly": "此连接无法修改设置，请在 Host 电脑配置。",

			"submission.title": "提交反馈",
			"submission.summary": "{count} 条对话 · 截取到序号 {seq} · 省略或截短 {omitted} 条",
			"submission.description": "提交包包含这条反馈及截至该回答的用户、助手对话文本。先查看内容，再决定是否发送到设置中的接收端。工具输出、系统提示和请求凭据不包含在提交包中。",
			"submission.refresh": "重新截取",
			"submission.download": "下载本地包",
			"submission.sending": "正在提交…",
			"submission.send": "发送到接收端",
			"submission.retry": "重试同一提交",
			"submission.localOnly": "远端提交未启用，内容仅保存在本地",
			"submission.preparing": "正在保存本地提交包…",
			"submission.preview": "查看提交内容",
			"submission.status.local": "已保存到本地",
			"submission.status.failed": "远端提交未确认",
			"submission.status.delivered": "接收端已确认",

			"action.like": "好的回答",
			"action.likeActive": "取消标记",
			"action.dislike": "有问题的回答",
			"action.dislikeActive": "取消标记",
			"note.open": "补充说明",
			"note.placeholder": "这条回答哪里好，或哪里有问题？（可选）",
			"note.save": "保存",
			"note.cancel": "取消",
			"note.aria": "反馈说明",
			"error.conflict": "这条反馈已在别处改动，已显示最新状态",
			"error.load": "反馈状态加载失败",
			"error.generic": "反馈保存失败"
		};
		/** English dictionary, checked complete against the zh key set. */
		const en = {
            "confirm.positive": "Confirm positive feedback",
            "confirm.negative": "Confirm negative feedback",
            "confirm.save": "Confirm feedback",
            "confirm.description": "Record feedback for this response. Category and note are optional; delivery to a recipient requires separate confirmation.",
            "confirm.saved": "Feedback recorded",
            "confirm.updated": "Feedback state updated",
            "confirm.tooLong": "The note is too long. Shorten it and retry.",
            "confirm.dismissError": "Dismiss error",

			"session.title":"Session feedback","session.recorded":"Feedback recorded","session.saving":"Saving…","session.description":"Record a remark about this session without starting a model request. Session sharing follows the current sharing settings.","session.category":"Category","session.choose":"Choose a category (optional)","session.placeholder":"Describe an issue or suggestion (optional)",
			"session.category.task-result":"Task result","session.category.instruction-following":"Instruction following","session.category.product-interaction":"Product interaction","session.category.service-stability":"Service stability","session.category.resource-cost":"Resources and cost","session.category.security-privacy-permission":"Security, privacy and permissions","session.category.other":"Other",
			"delivery.title": "Feedback delivery",
			"delivery.description": "Feedback is stored locally by default. Enabling a recipient still requires reviewing the package and choosing Send.",
			"delivery.enabled": "Allow delivery to recipient",
			"delivery.endpoint": "Recipient URL",
			"delivery.configured": "Configured",
			"delivery.unconfigured": "Not configured",
			"delivery.endpointHint": "Use HTTPS; loopback development endpoints may use HTTP. The URL is write-only. A blank draft preserves the stored URL.",
			"delivery.clear": "Clear URL",
			"delivery.loading": "Loading feedback settings…",
			"delivery.readonly": "Configure feedback delivery on the Host computer.",

			"submission.title": "Submit feedback",
			"submission.summary": "{count} messages · captured through sequence {seq} · {omitted} omitted or shortened",
			"submission.description": "The package contains this feedback and user/assistant text through the selected answer. Review it before sending to the configured recipient. Tool output, system prompts and request credentials are excluded.",
			"submission.refresh": "Capture again",
			"submission.download": "Download package",
			"submission.sending": "Sending…",
			"submission.send": "Send to recipient",
			"submission.retry": "Retry this submission",
			"submission.localOnly": "Remote delivery is disabled; stored locally only",
			"submission.preparing": "Saving local package…",
			"submission.preview": "Review package",
			"submission.status.local": "Saved locally",
			"submission.status.failed": "Remote delivery unconfirmed",
			"submission.status.delivered": "Recipient confirmed",

			"action.like": "Good response",
			"action.likeActive": "Remove rating",
			"action.dislike": "Bad response",
			"action.dislikeActive": "Remove rating",
			"note.open": "Add a note",
			"note.placeholder": "What was good, or what went wrong? (optional)",
			"note.save": "Save",
			"note.cancel": "Cancel",
			"note.aria": "Feedback note",
			"error.conflict": "This feedback changed elsewhere; the latest state is shown",
			"error.load": "Could not load feedback",
			"error.generic": "Could not save feedback"
		};
		//#endregion
		//#region lib/types/client/index.js
		/**
		* Message feedback plugin, browser half: the Like/Dislike entry in the
		* conversation.chat.assistant-actions strip. One MessageFeedbackController per
		* Session backs every message control in that Session, so a single list read
		* seeds the whole transcript. Mutations go through the generated
		* messageFeedback Remote; the Host owns per-item compare-and-set.
		* @module @deepseek-ai/dsh-client-ui-message-feedback/client
		*/
		/** Dictionary namespace owned by this plugin. */
		const NS = "feedback";
		/** Required services: the slot registry, the Remote namespace, and the copy. */
		const inject = [
			"slots",
			"remote",
			"remote.messageFeedback",
			"locale"
		];
		/**
		* Client plugin body: the per-message feedback entry and its per-session
		* object layer.
		* @param ctx - client root context.
		*/
		function apply(ctx) {
			ctx.slots.inject("conversation.session.header.utilities",()=>ctx.slots.register({name:"conversation.session.header.utilities",id:"session-feedback",order:30,locale:NS,inject:sessionId=>({sessionId})},props=>react.createElement(SessionFeedbackEntry,{...props,key:props.sessionId})));
			ctx.effect(() => ctx.locale.register(NS, {
				zh,
				en
			}), "ui-message-feedback: dictionaries");
			ctx.inject(["settingsScope"],scope=>{
				const settings=scope.settingsScope.bind({namespace:"feedback-delivery",decode:value=>({enabled:value?.enabled===true})});
				scope.slots.inject("settings.plugin.item",()=>scope.slots.register({name:"settings.plugin.item",id:"feedback-delivery",order:45,locale:NS},()=> (0,react_jsx_runtime.jsxs)("details",{className:"dshSettingsDisclosure",children:[(0,react_jsx_runtime.jsx)("summary",{children:scope.locale.bind(NS)("delivery.title")}), (0,react_jsx_runtime.jsx)(FeedbackDeliverySettings,{scope:settings,controls:scope.settingsScope.controls,t:scope.locale.bind(NS)})]})));
			});
			const controllers = /* @__PURE__ */ new Map();
			const call = async (method, payload) => {
				const rpcId = typeof globalThis.crypto?.randomUUID === "function" ? globalThis.crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
				const response = await fetch(`/api/messageFeedback.${method}`, {
					method: "POST",
					headers: { "content-type": "application/json" },
					body: JSON.stringify({ type: "client-request", rpcId, method: `messageFeedback.${method}`, payload })
				});
				if (!response.ok) throw new Error(`messageFeedback.${method} HTTP ${response.status}`);
				const envelope = await response.json();
				if (envelope.rpcId !== rpcId) throw new Error(`messageFeedback.${method} response id mismatch`);
				return envelope.result;
			};
			const remote = { list: (payload) => call("list", payload), put: (payload) => call("put", payload), delete: (payload) => call("delete", payload) };
			const controllerFor = (sessionId) => {
				let controller = controllers.get(sessionId);
				if (controller === void 0) {
					controller = new MessageFeedbackController(remote, sessionId);
					controllers.set(sessionId, controller);
				}
				return controller;
			};
			ctx.on("connection/reset", () => {
				for (const controller of controllers.values()) if (controller.getSnapshot().status !== "cold") controller.resync();
			});
			ctx.slots.inject("conversation.chat.assistant-actions", () => {
				const dispose = ctx.slots.register({
					name: "conversation.chat.assistant-actions",
					id: "feedback",
					order: 10,
					locale: NS,
					inject: (sessionId) => {
						const controller = controllerFor(sessionId);
						return {
							sessionId,
							hooks: { feedback: controller },
							ensure: () => controller.ensure(),
                            readItem: messageId => controller.getSnapshot().items.get(messageId),
                            retract: (messageId, rating) => controller.retract(messageId, rating),
                            confirmRating: (messageId, rating, note, category) => controller.confirmRating(messageId, rating, note, category),
							rate: (messageId, rating, note) => controller.rate(messageId, rating, note),
							toggle: (messageId, rating) => controller.toggle(messageId, rating),
							clearNote: (messageId) => controller.clearNote(messageId),
							clear: (messageId) => controller.clear(messageId)
						};
					}
				}, props => react.createElement(MessageFeedbackActions, { ...props, key: `${props.sessionId}:${props.messageId}` }));
				return () => {
					dispose();
					for (const controller of controllers.values()) controller.dispose();
					controllers.clear();
				};
			});
		}
		//#endregion
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map
