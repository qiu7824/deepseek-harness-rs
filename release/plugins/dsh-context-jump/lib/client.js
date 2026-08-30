window.__ModuleLoader__.load({
	id: "dsh-context-jump",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		//#region \0rolldown/runtime.js
		var __commonJSMin = (cb, mod) => () => (mod || (cb((mod = { exports: {} }).exports, mod), cb = null), mod.exports);
		//#endregion
		let react = require("react");
		let react_dom = require("react-dom");
		//#region src/client/locales.ts
		/** `userRail` namespace dictionaries. */
		/** Simplified Chinese dictionary (the key-set source of truth). */
		const zh = {
			"rail.aria": "你说过的话：{count} 条",
			"tick.aria": "跳到你说的话：{text}",
			"tick.imagesOnly": "（仅图片）",
			"tick.images": "含图片 {count} 张",
			"rail.targetError": "无法定位该消息，请重试"
		};
		/** English dictionary, checked complete against the zh key set. */
		const en = {
			"rail.aria": "Your messages: {count}",
			"tick.aria": "Jump to your message: {text}",
			"tick.imagesOnly": "(images only)",
			"tick.images": "{count} images",
			"rail.targetError": "Unable to locate this message. Please retry."
		};
		//#endregion
		//#region src/client/metrics.ts
		/** User-authored node renderer kinds: an ordinary message plus a message
		*  steered into an active turn are both the user's own words. */
		const USER_NODE_KINDS = /* @__PURE__ */ new Set(["user", "steering"]);
		const EMPTY_CONTENT = [];
		/**
		* Project a user message's blocks to plain text plus an image count.
		* @param content - the message's content blocks.
		* @returns concatenated plain text and image-block count.
		*/
		function messagePreview(content) {
			let text = "";
			let images = 0;
			for (const block of content) if (block.type === "text") text += block.text;
			else if (block.type === "image") images += 1;
			return {
				text,
				images
			};
		}
		/**
		* Collect the user's own messages from a chat snapshot in flow order.
		* @param order - the chat view's node order.
		* @param nodes - the chat view's node store.
		* @returns one {@link RailEntry} per user/steering node that the store holds.
		*/
		function userEntries(order, nodes) {
			const entries = [];
			for (const key of order) {
				const node = nodes.get(key);
				if (node === void 0 || !USER_NODE_KINDS.has(node.kind)) continue;
				const data = node.data;
				const preview = messagePreview(data?.content ?? EMPTY_CONTENT);
				entries.push({
					key,
					seq: node.anchorSeq,
					text: preview.text,
					images: preview.images
				});
			}
			return entries;
		}
		/**
		* Collapse whitespace and cap a message preview at `max` characters.
		* @param text - the plain message text.
		* @param max - hard cap in characters.
		* @returns the capped single-line-ish preview; short text is unchanged.
		*/
		function snippet(text, max) {
			const collapsed = text.replace(/\s+/g, " ").trim();
			return collapsed.length <= max ? collapsed : `${collapsed.slice(0, max)}…`;
		}
		/**
		* The maximum number of ticks that fit within a viewport height, so the rail
		* never overflows the screen. Older messages beyond the newest `this` count
		* are dropped, keeping the most recent ones.
		* @param viewportHeight - the viewport's pixel height.
		* @returns at least 1; messages beyond this are ignored.
		*/
		function maxVisibleTicks(viewportHeight) {
			const usable = Math.max(0, viewportHeight - 24);
			return Math.max(1, Math.floor((usable - 2) / 10) + 1);
		}
		/**
		* Tick line length (fisheye): the hovered/focused tick is the longest, direct
		* neighbors taper toward it, and far ticks rest at the base width — a length
		* change only, never a color change. Beyond distance 2 the length floors at
		* rest.
		* @param active - hovered/focused tick index, or null when none.
		* @param index - the tick's own index.
		* @returns the tick width in px.
		*/
		function tickLength(active, index) {
			if (active === null) return 12;
			const distance = Math.abs(index - active);
			if (distance === 0) return 26;
			return Math.max(12, 26 - 6 * distance);
		}
		/**
		* Scroll target that centers a row's flow offset in the scrollport.
		* @param port - scrollport metrics.
		* @param rowTopInPort - the row's top relative to the scrollport's visible box.
		* @param rowHeight - the row's rendered height.
		* @returns the clamped `scrollTop` that centers the row.
		*/
		function scrollCenterTarget(port, rowTopInPort, rowHeight) {
			const floor = Math.max(0, port.scrollHeight - port.clientHeight);
			const flowOffset = rowTopInPort + port.scrollTop;
			return Math.min(floor, Math.max(0, flowOffset - (port.clientHeight - rowHeight) / 2));
		}
		//#endregion
		//#region \0dsh-css:/workspace/dsh-user-message-rail/src/client/UserRail.module.css.mjs
		const css = "._6bmela_layer{z-index:10;pointer-events:none;width:20px;animation:.42s _6bmela_dsh-user-rail-fade-in;position:fixed;top:0;bottom:0}@media (pointer:coarse),(width<=767px){._6bmela_layer{display:none}}@keyframes _6bmela_dsh-user-rail-fade-in{0%{opacity:0}to{opacity:1}}._6bmela_tick{cursor:pointer;pointer-events:auto;background:0 0;border:0;height:10px;margin:0;padding:0;transition:width .12s;position:absolute;left:0;transform:translateY(-50%)}._6bmela_tick:before{content:\"\";background:var(--dsw-alias-label-tertiary);opacity:.5;border-radius:1px;width:100%;height:2px;transition:background-color .12s,opacity .12s;position:absolute;top:50%;left:0;transform:translateY(-50%)}._6bmela_tick[data-current]:before{background:var(--dsw-alias-label-primary);opacity:1}._6bmela_popover{z-index:100;background:var(--dsw-alias-tooltip-bg);width:max-content;max-width:320px;max-height:140px;color:var(--dsw-static-neutral-bluish-00);box-shadow:var(--dsw-shadow-lv3);pointer-events:auto;border-radius:8px;padding:8px 10px;position:absolute;left:calc(100% + 14px);overflow:hidden}._6bmela_popoverText{white-space:pre-line;overflow-wrap:break-word;font-size:13px;line-height:20px}._6bmela_popoverMeta{opacity:.72;margin-top:4px;font-size:12px;line-height:16px}";
		const tagId = "dsh-user-message-rail//workspace/dsh-user-message-rail/src/client/UserRail.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "dsh-user-message-rail";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var UserRail_module_css_default = {
			"layer": "_6bmela_layer",
			"popover": "_6bmela_popover",
			"popoverText": "_6bmela_popoverText",
			"dsh-user-rail-fade-in": "_6bmela_dsh-user-rail-fade-in",
			"popoverMeta": "_6bmela_popoverMeta",
			"tick": "_6bmela_tick"
		};
		//#endregion
		//#region node_modules/react/cjs/react-jsx-runtime.production.min.js
		/**
		* @license React
		* react-jsx-runtime.production.min.js
		*
		* Copyright (c) Facebook, Inc. and its affiliates.
		*
		* This source code is licensed under the MIT license found in the
		* LICENSE file in the root directory of this source tree.
		*/
		var require_react_jsx_runtime_production_min = /* @__PURE__ */ __commonJSMin(((exports) => {
			var f = require("react");
			var k = Symbol.for("react.element");
			var m = Object.prototype.hasOwnProperty;
			var n = f.__SECRET_INTERNALS_DO_NOT_USE_OR_YOU_WILL_BE_FIRED.ReactCurrentOwner;
			var p = {
				key: !0,
				ref: !0,
				__self: !0,
				__source: !0
			};
			function q(c, a, g) {
				var b, d = {}, e = null, h = null;
				void 0 !== g && (e = "" + g);
				void 0 !== a.key && (e = "" + a.key);
				void 0 !== a.ref && (h = a.ref);
				for (b in a) m.call(a, b) && !p.hasOwnProperty(b) && (d[b] = a[b]);
				if (c && c.defaultProps) for (b in a = c.defaultProps, a) void 0 === d[b] && (d[b] = a[b]);
				return {
					$$typeof: k,
					type: c,
					key: e,
					ref: h,
					props: d,
					_owner: n.current
				};
			}
			exports.jsx = q;
			exports.jsxs = q;
		}));
		//#endregion
		//#region src/client/UserRail.tsx
		var import_jsx_runtime = (/* @__PURE__ */ __commonJSMin(((exports, module) => {
			module.exports = require_react_jsx_runtime_production_min();
		})))();
		/** Preview cap: a very long message stays a summary in the popover. */
		const PREVIEW_MAX = 200;
		/** Accessible tick-label cap; the popover carries the full preview. */
		const SNIPPET_MAX = 64;
		/** Stable absent projection snapshot required by useSyncExternalStore. */
		const EMPTY_RAIL_ENTRIES = [];
		/** Popover edge clearance inside the window. */
		const POPOVER_EDGE = 8;
		/** Find the settled flow row for one node key (keys are opaque ids, so match
		*  by dataset value instead of interpolating an attribute selector). */
		function findRow(flow, key) {
			for (const row of flow.querySelectorAll("[data-chat-anchor-key]")) if (row.dataset.chatAnchorKey === key) return row;
			return null;
		}
		/** Locate the active conversation scrollport and its chat flow, when mounted. */
		function conversationBox() {
			const port = document.querySelector("[data-conversation-scroll]");
			const flow = port?.querySelector("[data-chat-flow]") ?? null;
			return port === null || flow === null ? null : {
				port,
				flow
			};
		}
		/** The strip's left edge: inset from the chat box's left face so the whole
		*  rail sits inside the chat window, its tick line aligned with the window's
		*  top title bar. */
		function stripLeft(port) {
			return port.getBoundingClientRect().left + 20;
		}
		/**
		* The rail entry: pure presentation over the session kit — every datum
		* arrives through the props shares (the framework session hooks and the
		* locale seat); the component touches no ctx.
		*/
		function UserRail({ useSession, t, railFace, ensureTarget }) {
			const order = useSession((s) => s.chat.order);
			const nodes = useSession((s) => s.chat.nodes);
			const indexed = (0, react.useSyncExternalStore)(
				railFace.subscribe,
				railFace.getSnapshot,
				railFace.getSnapshot
			) ?? [];
			const [railHeight, setRailHeight] = (0, react.useState)(() => window.innerHeight);
			const max = maxVisibleTicks(railHeight);
			const allEntries = (0, react.useMemo)(() => Array.isArray(indexed) && indexed.length > 0 ? indexed : userEntries(order, nodes), [indexed, order, nodes]);
			const [windowStart, setWindowStart] = (0, react.useState)(0);
			(0, react.useEffect)(() => {
				setWindowStart(Math.max(0, allEntries.length - max));
			}, [allEntries.length, max]);
			const entries = (0, react.useMemo)(() => allEntries.slice(windowStart, windowStart + max), [allEntries, windowStart, max]);
			const pageTicks = (direction) => setWindowStart((current) => Math.max(0, Math.min(Math.max(0, allEntries.length - max), current + direction * Math.max(1, max - 1))));
			const entriesRef = (0, react.useRef)(entries);
			entriesRef.current = entries;
			const [active, setActive] = (0, react.useState)(null);
			const [current, setCurrent] = (0, react.useState)(null);
			const [targetError, setTargetError] = (0, react.useState)(null);
			const [box, setBox] = (0, react.useState)(() => conversationBox());
			const boxRef = (0, react.useRef)(box);
			boxRef.current = box;
			const [railLeft, setRailLeft] = (0, react.useState)(() => {
				const initial = conversationBox();
				return initial === null ? null : stripLeft(initial.port);
			});
			const railLeftRef = (0, react.useRef)(railLeft);
			railLeftRef.current = railLeft;
			const railHeightRef = (0, react.useRef)(railHeight);
			railHeightRef.current = railHeight;
			const currentRef = (0, react.useRef)(current);
			currentRef.current = current;
			const [popoverTop, setPopoverTop] = (0, react.useState)(null);
			const popoverRef = (0, react.useRef)(null);
			const rafRef = (0, react.useRef)(0);
			const ready = entries.length > 0 && box !== null && railLeft !== null;
			/** The viewport's vertical center: the whole tick set is symmetric around it. */
			const railCenter = railHeight / 2;
			/** Half the set's height: offsets the group so the first and last lines
			*  straddle the center (the set is centered, not the first line). */
			const groupOffset = Math.max(0, entries.length - 1) * 10 / 2;
			/** One tick's vertical center, with the whole set centered on the viewport. */
			const tickCenterY = (0, react.useCallback)((index) => railCenter - groupOffset + index * 10, [railCenter, groupOffset]);
			/** Re-read the chat box/left edge and re-derive the message at the viewport
			*  center, so the scroll highlight follows the conversation. */
			const refresh = (0, react.useCallback)(() => {
				const next = conversationBox();
				const previousBox = boxRef.current;
				if (!(previousBox !== null && next !== null && previousBox.port === next.port && previousBox.flow === next.flow) && previousBox !== next) {
					boxRef.current = next;
					setBox(next);
				}
				const nextLeft = next === null ? null : stripLeft(next.port);
				if (railLeftRef.current !== nextLeft) {
					railLeftRef.current = nextLeft;
					setRailLeft(nextLeft);
				}
				const nextHeight = window.innerHeight;
				if (railHeightRef.current !== nextHeight) {
					railHeightRef.current = nextHeight;
					setRailHeight(nextHeight);
				}
				if (next === null) {
					if (currentRef.current !== null) {
						currentRef.current = null;
						setCurrent(null);
					}
					return;
				}
				const { port, flow } = next;
				const viewportCenter = port.getBoundingClientRect().top + port.clientHeight / 2;
				let best = null;
				let bestDist = Infinity;
				entriesRef.current.forEach((entry, index) => {
					const row = findRow(flow, entry.key);
					if (row === null) return;
					const rect = row.getBoundingClientRect();
					const rowCenter = rect.top + rect.height / 2;
					const dist = Math.abs(rowCenter - viewportCenter);
					if (dist < bestDist) {
						bestDist = dist;
						best = index;
					}
				});
				if (currentRef.current !== best) {
					currentRef.current = best;
					setCurrent(best);
				}
			}, []);
			/** Coalesce any number of triggers into one layout + highlight per frame. */
			const scheduleRefresh = (0, react.useCallback)(() => {
				if (rafRef.current !== 0) return;
				rafRef.current = requestAnimationFrame(() => {
					rafRef.current = 0;
					refresh();
				});
			}, [refresh]);
			(0, react.useEffect)(() => {
				const observer = new MutationObserver(scheduleRefresh);
				observer.observe(document.body, {
					childList: true,
					subtree: true
				});
				return () => observer.disconnect();
			}, [scheduleRefresh]);
			(0, react.useEffect)(() => {
				window.addEventListener("resize", scheduleRefresh);
				return () => window.removeEventListener("resize", scheduleRefresh);
			}, [scheduleRefresh]);
			(0, react.useEffect)(() => {
				const current = boxRef.current;
				if (current === null || typeof ResizeObserver === "undefined") return;
				const observer = new ResizeObserver(scheduleRefresh);
				observer.observe(current.port);
				return () => observer.disconnect();
			}, [box, scheduleRefresh]);
			(0, react.useEffect)(() => {
				const current = boxRef.current;
				if (current === null) return;
				const onScroll = () => scheduleRefresh();
				current.port.addEventListener("scroll", onScroll, { passive: true });
				return () => current.port.removeEventListener("scroll", onScroll);
			}, [box, scheduleRefresh]);
			(0, react.useEffect)(() => {
				scheduleRefresh();
			}, [scheduleRefresh, ready, entries]);
			/** Keep the popover below and right of the active tick (top-aligned with the
			*  tick line) and inside the window. */
			(0, react.useLayoutEffect)(() => {
				const el = popoverRef.current;
				if (el === null || active === null) return;
				const tickTop = tickCenterY(active) - 1;
				const height = Math.max(0, el.offsetHeight);
				setPopoverTop(Math.min(Math.max(POPOVER_EDGE, railHeight - height - POPOVER_EDGE), Math.max(POPOVER_EDGE, tickTop)));
			}, [
				active,
				tickCenterY,
				railHeight
			]);
			/** Scroll the conversation to center the activated message. */
			const activate = (0, react.useCallback)(async (index) => {
				const entry = entriesRef.current[index];
				if (entry === void 0) return;
				setTargetError(null);
				let current = boxRef.current;
				if (current === null || !current.port.isConnected || !current.flow.isConnected) return;
				let targetKey = entry.key;
				let row = findRow(current.flow, targetKey);
				if (row === null && Number.isSafeInteger(entry.seq)) {
					targetKey = await ensureTarget(entry.seq, entry.key);
					if (targetKey === null) {
						setTargetError(t("rail.targetError"));
						return;
					}
					for (let frame = 0; frame < 12; frame++) {
						await new Promise((resolve) => requestAnimationFrame(resolve));
						current = conversationBox();
						if (current === null) continue;
						boxRef.current = current;
						row = findRow(current.flow, targetKey);
						if (row !== null) break;
					}
				}
				if (row === null) {
					setTargetError(t("rail.targetError"));
					return;
				}
				const rect = row.getBoundingClientRect();
				const portRect = current.port.getBoundingClientRect();
				const target = scrollCenterTarget(current.port, rect.top - portRect.top, rect.height);
				if (typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches) current.port.scrollTop = target;
				else current.port.scrollTo({ top: target, behavior: "smooth" });
			}, [ensureTarget, t]);
			if (!ready) return null;
			const count = allEntries.length;
			return (0, react_dom.createPortal)(/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", {
				className: UserRail_module_css_default.layer,
				style: { left: railLeft },
				tabIndex: 0,
				"aria-label": `${t("rail.aria", { count })}，当前 ${windowStart + 1}-${windowStart + entries.length}`,
				onWheel: (event) => {
					event.preventDefault();
					pageTicks(event.deltaY < 0 ? -1 : 1);
				},
				onKeyDown: (event) => {
					if (event.key === "PageUp") { event.preventDefault(); pageTicks(-1); }
					if (event.key === "PageDown") { event.preventDefault(); pageTicks(1); }
				},
				children: [targetError !== null && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", {
					role: "alert",
					style: { position: "fixed", left: railLeft + 28, top: railCenter - 18, zIndex: 101, padding: "7px 10px", borderRadius: 6, background: "var(--dsw-alias-tooltip-bg)", color: "var(--dsw-static-neutral-bluish-00)", fontSize: 12, whiteSpace: "nowrap" },
					children: targetError
				}), entries.map((entry, index) => {
					const label = snippet(entry.text, SNIPPET_MAX);
					return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("button", {
						type: "button",
						className: UserRail_module_css_default.tick,
						style: {
							top: `${tickCenterY(index)}px`,
							width: tickLength(active, index)
						},
						"data-active": active === index || void 0,
						"data-current": (active !== null ? active : current) === index || void 0,
						"aria-label": label === "" ? t("tick.imagesOnly") : t("tick.aria", { text: label }),
						onClick: () => activate(index),
						onMouseEnter: () => setActive(index),
						onMouseLeave: () => setActive(null),
						onFocus: () => setActive(index),
						onBlur: () => setActive(null)
					}, entry.key);
				}), active !== null && active < entries.length && (() => {
					const entry = entries[active];
					if (entry === void 0) return null;
					return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(MessagePopover, {
						entry,
						top: popoverTop,
						t,
						popoverRef,
						onClose: () => setActive(null)
					});
				})()]
			}), document.body);
		}
		/** One message preview card, portaled beside the hovered tick. */
		function MessagePopover({ entry, top, t, popoverRef, onClose }) {
			const preview = snippet(entry.text, PREVIEW_MAX);
			return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", {
				ref: popoverRef,
				role: "tooltip",
				className: UserRail_module_css_default.popover,
				style: { top: top ?? void 0 },
				onMouseLeave: onClose,
				children: [
					preview !== "" && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", {
						className: UserRail_module_css_default.popoverText,
						children: preview
					}),
					entry.images > 0 && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", {
						className: UserRail_module_css_default.popoverMeta,
						children: t("tick.images", { count: entry.images })
					}),
					preview === "" && entry.images === 0 && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", {
						className: UserRail_module_css_default.popoverMeta,
						children: t("tick.imagesOnly")
					})
				]
			});
		}
		//#endregion
		//#region src/client/index.ts
		/** Dictionary namespace owned by this plugin. */
		const NS = "userRail";
		/** Required services: the slot registry, the rail copy, and the session scope. */
		const inject = [
			"slots",
			"locale",
			"sessions"
		];
		/**
		* Client plugin body: register the `userRail` dictionaries and the rail
		* component into the composer chain's overlay seat. The complete lightweight
		* index comes from a projection; an old target loads only its containing page.
		* @param ctx - client root context.
		*/
		function apply(ctx) {
			ctx.effect(() => ctx.locale.register(NS, {
				zh,
				en
			}), "dsh-user-message-rail: dictionaries");
			ctx.slots.inject("conversation.input.overlay", () => ctx.slots.register({
				name: "conversation.input.overlay",
				id: "user-message-rail",
				order: 10,
				locale: NS,
				inject: (sessionId) => {
					const currentSession = () => ctx.sessions.binding(sessionId)?.session;
					const currentFace = () => currentSession()?.projections.faceOf("userMessageRail");
					const railFace = {
						subscribe: (listener) => {
							let face = currentFace();
							let unsubscribeFace = face?.subscribe(listener) ?? (() => {});
							const unsubscribeSession = currentSession()?.subscribe(() => {
								const next = currentFace();
								if (next !== face) {
									unsubscribeFace();
									face = next;
									unsubscribeFace = face?.subscribe(listener) ?? (() => {});
								}
								listener();
							}) ?? (() => {});
							return () => {
								unsubscribeFace();
								unsubscribeSession();
							};
						},
						getSnapshot: () => currentFace()?.getSnapshot() ?? EMPTY_RAIL_ENTRIES
					};
					const nodeKeyAt = (session, seq, preferredKey) => {
						const snapshot = session.getSnapshot();
						if (snapshot.chat.nodes.get(preferredKey) !== void 0) return preferredKey;
						for (const key of snapshot.chat.order) {
							const node = snapshot.chat.nodes.get(key);
							if (node?.anchorSeq === seq && node.kind === "user") return key;
						}
						return null;
					};
					return {
						railFace,
						ensureTarget: async (seq, key) => {
							const session = currentSession();
							if (session === void 0) return null;
							const existing = nodeKeyAt(session, seq, key);
							if (existing !== null) return existing;
							const loaded = await session.loadAround(seq, true);
							await new Promise((resolve) => setTimeout(resolve, 0));
							const found = loaded ? nodeKeyAt(session, seq, key) : null;
							return found;
						}
					};
				}
			}, UserRail));
		}
		//#endregion
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map