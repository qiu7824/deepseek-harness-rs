/**
 * dsh-composer-expand — client half.
 *
 * A button in the composer tool row (`conversation.input.right`, the
 * official seat for clickable things next to the send button). Clicking
 * toggles a CSS class on the conversation scrollport that grows the
 * textarea + composer stack to a tall writing view; clicking again
 * restores the default capped height. The choice persists per browser
 * via localStorage so it survives reloads and workspace switches.
 *
 * UI text is localized through the harness `locale` service (zh + en).
 *
 * How the height toggle works:
 * The conversation scrollport carries `data-conversation-scroll` and
 * hosts the sticky composer stack. Inside it, `data-composer-seat`
 * marks the actual composer card (the `data-conversation-composer-overlay`
 * anchor only exists in the Trajectory view, not the default ChatView).
 * Toggling a class on the scrollport flips a CSS variable that raises
 * the composer's `max-height` from the harness default (~141px scrollport
 * cap on the disclosure body) to a tall view, so long drafts no longer
 * scroll inside a tiny window. The 141px cap is a documented DSH design
 * choice for the disclosure rows; this plugin only affects the composer
 * textarea container, not the disclosure rows.
 */
import { createElement, useCallback, useEffect, useState } from 'react'

// Slot context + locale for i18n.
export const inject = ['slots', 'locale']

const ID = 'dsh-composer-expand'
const STORAGE_KEY = ID + ':expanded'
const CSS_VAR_HEIGHT = 'dsh-composer-expand-height'
const CLASS_ON = 'dsh-composer-expand-on'
const TALL_HEIGHT = '70vh'

const CSS = [
  '.' + CLASS_ON + ' { --' + CSS_VAR_HEIGHT + ': ' + TALL_HEIGHT + '; }',
  // The composer card sits inside [data-composer-seat], which is a direct
  // child of [data-conversation-scroll] in the default ChatView. The
  // [data-conversation-composer-overlay] anchor only exists in the
  // Trajectory view, so it is not used here.
  // Uncap the REAL height limit: the harness seat sets
  // --dsh-composer-text-max-height (336px) which caps the textarea
  // scrollport (.uV2eYG_scroll { max-height: var(...) }); overriding the
  // variable on the seat relaxes that cap. max-height on the seat itself
  // has no effect on the inner scrollport.
  '.' + CLASS_ON + ' [data-composer-seat] { --dsh-composer-text-max-height: var(--' + CSS_VAR_HEIGHT + ') !important; }',
  // Stretch the input area even with little content: the hidden mirror
  // (data-input-mirror) is what actually sizes the textarea wrapper.
  '.' + CLASS_ON + ' [data-input-mirror] { min-height: 300px !important; }',
  // Belt-and-braces: keep the textarea itself under the tall cap.
  '.' + CLASS_ON + ' [data-composer-seat] textarea { max-height: var(--' + CSS_VAR_HEIGHT + ') !important; }',
  '.cpex-btn { display: inline-flex; align-items: center; gap: 4px; height: 24px; border: 1px solid rgba(127,127,137,.35); background: transparent; color: inherit; border-radius: 999px; padding: 0 9px; font-size: 12px; line-height: 1; cursor: pointer; opacity: .75; white-space: nowrap; font-variant-numeric: tabular-nums; transition: opacity .12s ease, background-color .12s ease; }',
  '.cpex-btn:hover { opacity: 1; background: rgba(127,127,137,.12); }',
  '.cpex-btn[data-on="true"] { opacity: 1; background: rgba(127,127,137,.18); border-color: rgba(127,127,137,.6); }',
  '.cpex-glyph { font-size: 13px; line-height: 1; }',
].join('\n')

/** One <style data-plugin> tag per load; the loader removes plugin-owned tags on unload. */
function injectStyle() {
  const tagId = ID + '/button.css';
  if (typeof document !== 'undefined' && document.querySelector('style[data-plugin-css="' + tagId + '"]') === null) {
    const tag = document.createElement('style');
    tag.dataset.plugin = ID;
    tag.dataset.pluginCss = tagId;
    tag.textContent = CSS;
    document.head.appendChild(tag);
  }
}

const ZH = {
  'button.collapse.label': '收起',
  'button.collapse.title': '恢复输入框默认高度',
  'button.expand.label': '展开',
  'button.expand.title': '把输入框高度扩大到 ' + TALL_HEIGHT + '，方便写长 prompt',
};
const EN = {
  'button.collapse.label': 'Collapse',
  'button.collapse.title': 'Restore the composer to its default height',
  'button.expand.label': 'Expand',
  'button.expand.title': 'Grow the composer to ' + TALL_HEIGHT + ' so long drafts stay readable',
};

function readStored() {
  try { return localStorage.getItem(STORAGE_KEY) === '1'; } catch (_) { return false; }
}
function writeStored(on) {
  try { localStorage.setItem(STORAGE_KEY, on ? '1' : '0'); } catch (_) { /* private mode */ }
}

/**
 * In expanded mode a plain Enter inserts a newline instead of triggering
 * the harness send handler. Modifier shortcuts remain available for send.
 * The listener is attached to the actual editor nodes so it runs before
 * React's delegated keydown handler, while leaving the browser default
 * newline behavior untouched.
 */
function bindExpandedEnterBehavior() {
  if (typeof document === 'undefined') return;

  const seat = document.querySelector('[data-composer-seat]');
  if (!seat) return;

  const bound = new Set();
  const handleKeyDown = function (event) {
    if (event.key !== 'Enter' || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return;
    event.stopPropagation();
  };
  const bind = function () {
    seat.querySelectorAll('textarea, [contenteditable="true"]').forEach(function (editor) {
      if (bound.has(editor)) return;
      editor.addEventListener('keydown', handleKeyDown);
      bound.add(editor);
    });
  };

  bind();
  const observer = typeof MutationObserver === 'undefined' ? null : new MutationObserver(bind);
  if (observer) observer.observe(seat, { childList: true, subtree: true });

  return function () {
    if (observer) observer.disconnect();
    bound.forEach(function (editor) { editor.removeEventListener('keydown', handleKeyDown); });
  };
}

export function apply(ctx) {
  injectStyle();

  let t = function (key) { return key; };
  try {
    ctx.locale.register(ID, 'zh', ZH);
    ctx.locale.register(ID, 'en', EN);
    t = ctx.locale.bind(ID);
  } catch (error) {
    console.error(ID + ': locale registration failed: ' + String(error));
  }

  function ExpandButton(props) {
    const sessionId = props.sessionId ? String(props.sessionId) : '';
    const [on, setOn] = useState(readStored());
    const [, setLocaleTick] = useState(0);

    useEffect(function () {
      return ctx.locale.subscribe(function () { setLocaleTick(function (x) { return x + 1; }); });
    }, []);

    useEffect(function () {
      if (typeof document === 'undefined') return;
      const root = document.querySelector('[data-conversation-scroll]');
      if (root) root.classList.toggle(CLASS_ON, on);
      writeStored(on);
    }, [on]);

    useEffect(function () {
      if (!on) return;
      return bindExpandedEnterBehavior();
    }, [on]);

    const onClick = useCallback(function () { setOn(function (v) { return !v; }); }, []);
    const label = on ? t('button.collapse.label') : t('button.expand.label');
    const title = on ? t('button.collapse.title') : t('button.expand.title');

    return createElement('button', {
      type: 'button',
      className: 'cpex-btn',
      title: title,
      'aria-pressed': on ? 'true' : 'false',
      'data-on': on ? 'true' : 'false',
      onClick: onClick,
    },
      createElement('span', { className: 'cpex-glyph', 'aria-hidden': 'true' }, on ? '⬇' : '⬆'),
      createElement('span', null, label),
    );
  }

  ctx.slots.inject('conversation.input.right', function () {
    return ctx.slots.register(
      { name: 'conversation.input.right', id: 'composer-expand', order: 90, label: 'Expand composer' },
      function (props) { return createElement(ExpandButton, props); },
    );
  });
}