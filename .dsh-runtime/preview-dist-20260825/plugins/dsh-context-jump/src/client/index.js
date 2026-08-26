import { createElement, useEffect, useLayoutEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'

export const inject = ['slots']

const ID = 'dsh-context-jump'
const CSS = `
.ctxjump-host{position:fixed;z-index:12;pointer-events:none;width:34px;overflow:visible}
.ctxjump-rail{position:absolute;inset:0;pointer-events:none}
.ctxjump-line{position:absolute;left:16px;top:12px;bottom:12px;width:1px;background:var(--dsw-alias-border-l2,rgba(127,127,137,.22));border-radius:1px}
.ctxjump-mark{position:absolute;left:10px;width:13px;height:10px;padding:0;border:0;background:transparent;pointer-events:auto;cursor:pointer;transform:translateY(-50%)}
.ctxjump-mark::before{content:"";position:absolute;left:4px;top:4px;width:7px;height:1px;border-radius:2px;background:var(--dsw-alias-label-caption,#98a2b3);opacity:.55;transition:width .12s ease,left .12s ease,height .12s ease,opacity .12s ease,background-color .12s ease}
.ctxjump-mark:hover::before,.ctxjump-mark:focus-visible::before{left:1px;width:13px;height:2px;opacity:1;background:var(--dsw-alias-label-primary,#344054)}
.ctxjump-mark[data-active="true"]::before{left:0;width:15px;height:2px;opacity:1;background:var(--dsw-alias-label-primary,#101828)}
.ctxjump-mark:focus-visible{outline:2px solid var(--dsw-alias-interactive-primary,#175cd3);outline-offset:2px;border-radius:4px}
.ctxjump-tip{position:absolute;left:31px;top:50%;display:none;width:max-content;max-width:min(300px,calc(100vw - 90px));padding:9px 11px;border:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.2));border-radius:10px;background:var(--dsw-specific-menu,var(--dsw-alias-bg-base,#fff));box-shadow:var(--dsw-shadow-lv2,0 6px 20px rgba(16,24,40,.12));color:var(--dsw-alias-label-primary,#101828);font-size:12px;line-height:18px;white-space:normal;transform:translateY(-50%);pointer-events:none}
.ctxjump-mark:hover .ctxjump-tip,.ctxjump-mark:focus-visible .ctxjump-tip{display:block}
.ctxjump-position{display:block;margin-top:2px;color:var(--dsw-alias-label-caption,#98a2b3);font-size:11px;font-variant-numeric:tabular-nums}
.ctxjump-edge{position:absolute;left:7px;width:19px;height:19px;padding:0;border:0;border-radius:6px;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-tertiary,#667085);box-shadow:0 0 0 1px var(--dsw-alias-border-l2,rgba(127,127,137,.18));pointer-events:auto;cursor:pointer;font-size:13px;line-height:19px;opacity:0;transition:opacity .12s ease,background-color .12s ease}
.ctxjump-host:hover .ctxjump-edge,.ctxjump-edge:focus-visible{opacity:1}
.ctxjump-edge:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}
.ctxjump-edge[data-edge="top"]{top:0;transform:translateY(-50%)}
.ctxjump-edge[data-edge="bottom"]{bottom:0;transform:translateY(50%)}
@media (max-width:700px){.ctxjump-host{display:none}}
@media (prefers-reduced-motion:reduce){.ctxjump-mark::before,.ctxjump-edge{transition:none}}
`

function installStyle() {
  if (document.querySelector('style[data-plugin-css="' + ID + '/client.css"]')) return
  const style = document.createElement('style')
  style.dataset.plugin = ID
  style.dataset.pluginCss = ID + '/client.css'
  style.textContent = CSS
  document.head.appendChild(style)
}

function readConversation() {
  const scroll = document.querySelector('[data-conversation-scroll]')
  if (!(scroll instanceof HTMLElement)) return { scroll: null, rows: [] }
  const rows = [...scroll.querySelectorAll('[data-chat-flow-kind="user"]')]
    .filter((row) => row instanceof HTMLElement)
  return { scroll, rows }
}

function titleOf(row, index) {
  const text = (row.innerText || '').trim().split('\n').find(Boolean)?.trim()
  return (text || `对话 ${index + 1}`).slice(0, 120)
}

function rowTop(row, scroll) {
  return row.getBoundingClientRect().top - scroll.getBoundingClientRect().top + scroll.scrollTop
}

function jumpTo(row, scroll) {
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches
  scroll.scrollTo({ top: Math.max(0, rowTop(row, scroll) - 12), behavior: reduce ? 'auto' : 'smooth' })
}

function ContextJumpRail() {
  const [snapshot, setSnapshot] = useState({ scroll: null, rows: [], current: -1, rect: null })

  useLayoutEffect(() => {
    let boundScroll = null
    let frame = 0
    const observer = new MutationObserver(() => schedule())

    const refresh = () => {
      frame = 0
      const { scroll, rows } = readConversation()
      if (boundScroll !== scroll) {
        boundScroll?.removeEventListener('scroll', schedule)
        observer.disconnect()
        boundScroll = scroll
        boundScroll?.addEventListener('scroll', schedule, { passive: true })
        if (boundScroll) observer.observe(boundScroll, { childList: true, subtree: true })
      }
      if (!scroll || rows.length === 0) {
        setSnapshot({ scroll, rows: [], current: -1, rect: null })
        return
      }
      const rect = scroll.getBoundingClientRect()
      const visibleTop = rect.top + 16
      let current = rows.findIndex((row) => row.getBoundingClientRect().bottom > visibleTop)
      if (current < 0) current = rows.length - 1
      setSnapshot({
        scroll,
        rows,
        current,
        rect: { left: rect.left, top: rect.top + 18, height: Math.max(80, rect.height - 36) },
      })
    }
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(refresh)
    }

    refresh()
    window.addEventListener('resize', schedule)
    return () => {
      if (frame) cancelAnimationFrame(frame)
      boundScroll?.removeEventListener('scroll', schedule)
      observer.disconnect()
      window.removeEventListener('resize', schedule)
    }
  }, [])

  const marks = useMemo(() => {
    const { scroll, rows } = snapshot
    if (!scroll || rows.length === 0) return []
    const maxTop = Math.max(1, scroll.scrollHeight - scroll.clientHeight)
    return rows.map((row, index) => ({
      row,
      title: titleOf(row, index),
      top: Math.max(4, Math.min(96, rowTop(row, scroll) / maxTop * 92 + 4)),
    }))
  }, [snapshot.scroll, snapshot.rows, snapshot.rect])

  useEffect(() => {
    if (!snapshot.scroll) return
    const onKeyDown = (event) => {
      if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
      if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return
      const offset = event.key === 'ArrowUp' ? -1 : 1
      const target = marks[Math.max(0, Math.min(marks.length - 1, snapshot.current + offset))]
      if (!target) return
      event.preventDefault()
      jumpTo(target.row, snapshot.scroll)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [marks, snapshot.current, snapshot.scroll])

  if (!snapshot.scroll || !snapshot.rect || marks.length === 0) return null
  const hostStyle = {
    left: Math.max(0, snapshot.rect.left + 2) + 'px',
    top: snapshot.rect.top + 'px',
    height: snapshot.rect.height + 'px',
  }
  return createPortal(createElement('nav', {
    className: 'ctxjump-host',
    style: hostStyle,
    'aria-label': '对话翻页',
  },
    createElement('div', { className: 'ctxjump-rail' },
      createElement('div', { className: 'ctxjump-line', 'aria-hidden': 'true' }),
      createElement('button', {
        type: 'button', className: 'ctxjump-edge', 'data-edge': 'top', title: '回到顶部',
        onClick: () => snapshot.scroll.scrollTo({ top: 0, behavior: 'smooth' }),
      }, '↑'),
      marks.map((mark, index) => createElement('button', {
        key: mark.row.dataset.chatAnchorKey || index,
        type: 'button',
        className: 'ctxjump-mark',
        style: { top: mark.top + '%' },
        'data-active': index === snapshot.current ? 'true' : 'false',
        'aria-label': `跳转到第 ${index + 1} 条：${mark.title}`,
        'aria-current': index === snapshot.current ? 'location' : undefined,
        onClick: () => jumpTo(mark.row, snapshot.scroll),
      }, createElement('span', { className: 'ctxjump-tip' },
        mark.title,
        createElement('span', { className: 'ctxjump-position' }, `${index + 1} / ${marks.length}`),
      ))),
      createElement('button', {
        type: 'button', className: 'ctxjump-edge', 'data-edge': 'bottom', title: '回到底部',
        onClick: () => snapshot.scroll.scrollTo({ top: snapshot.scroll.scrollHeight, behavior: 'smooth' }),
      }, '↓'),
    ),
  ), document.body)
}

export function apply(ctx) {
  installStyle()
  ctx.slots.inject('conversation.session.header.utilities', () => ctx.slots.register({
    name: 'conversation.session.header.utilities',
    id: 'context-jump-rail',
    order: 1000,
    label: 'Context jump rail',
  }, ContextJumpRail))
}
