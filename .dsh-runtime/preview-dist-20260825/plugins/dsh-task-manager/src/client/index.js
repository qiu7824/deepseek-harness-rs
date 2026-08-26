import { createElement, useEffect, useId, useMemo, useRef, useState } from 'react'

export const inject = ['slots', 'sessions']

const ID = 'dsh-task-manager'
const CSS = `
.dshtm-root{position:relative;display:inline-flex}
.dshtm-trigger{position:relative;display:inline-grid;place-items:center;width:28px;height:28px;padding:0;border:0;border-radius:8px;background:transparent;color:var(--dsw-alias-label-secondary,#667085);cursor:pointer;transition:background-color .12s ease,color .12s ease}
.dshtm-trigger:hover,.dshtm-trigger:focus-visible,.dshtm-trigger[data-open="true"]{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}
.dshtm-trigger:focus-visible{outline:2px solid var(--dsw-alias-interactive-primary,#175cd3);outline-offset:1px}
.dshtm-trigger svg{width:18px;height:18px}
.dshtm-badge{position:absolute;right:-3px;top:-4px;min-width:14px;height:14px;padding:0 3px;border:2px solid var(--dsw-alias-bg-base,#fff);border-radius:8px;background:var(--dsw-alias-interactive-primary,#175cd3);color:#fff;font:600 9px/10px Inter,sans-serif;text-align:center;box-sizing:border-box}
.dshtm-popover{position:absolute;right:0;bottom:100%;z-index:80;width:min(390px,calc(100vw - 24px));max-height:min(440px,65vh);display:flex;flex-direction:column;border:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.22));border-radius:14px;background:var(--dsw-specific-menu,var(--dsw-alias-bg-base,#fff));box-shadow:var(--dsw-shadow-lv3,0 12px 36px rgba(16,24,40,.18));color:var(--dsw-alias-label-primary,#101828);overflow:hidden}
.dshtm-head{display:flex;align-items:center;gap:10px;padding:13px 14px 10px;border-bottom:1px solid var(--dsw-alias-border-l1,rgba(127,127,137,.14))}
.dshtm-title{min-width:0;flex:1;font-size:14px;font-weight:600}
.dshtm-summary{font-size:12px;color:var(--dsw-alias-label-tertiary,#667085);font-variant-numeric:tabular-nums;white-space:nowrap}
.dshtm-progress{height:3px;background:var(--dsw-alias-bg-layer-2,rgba(127,127,137,.12))}
.dshtm-progress>span{display:block;height:100%;background:var(--dsw-alias-state-success-primary,#12b76a);transition:width .2s ease}
.dshtm-list{margin:0;padding:5px 7px;list-style:none;overflow:auto}
.dshtm-row{display:grid;grid-template-columns:18px minmax(0,1fr) auto;align-items:center;gap:8px;min-height:36px;padding:4px 5px;border-radius:8px}
.dshtm-row:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.08))}
.dshtm-status{display:grid;place-items:center;width:18px;height:18px;color:var(--dsw-alias-label-caption,#98a2b3);font-size:13px}
.dshtm-row[data-status="completed"] .dshtm-status{color:var(--dsw-alias-state-success-primary,#12b76a)}
.dshtm-row[data-status="in_progress"] .dshtm-status{color:var(--dsw-alias-interactive-primary,#175cd3)}
.dshtm-content{min-width:0;font-size:13px;line-height:19px;word-break:break-word}
.dshtm-row[data-status="completed"] .dshtm-content{color:var(--dsw-alias-label-tertiary,#667085);text-decoration:line-through}
.dshtm-actions{display:flex;align-items:center;gap:2px;opacity:0;transition:opacity .12s ease}
.dshtm-row:hover .dshtm-actions,.dshtm-row:focus-within .dshtm-actions{opacity:1}
.dshtm-action{display:grid;place-items:center;width:25px;height:25px;padding:0;border:0;border-radius:6px;background:transparent;color:var(--dsw-alias-label-tertiary,#667085);font-size:13px;cursor:pointer}
.dshtm-action:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.14));color:var(--dsw-alias-label-primary,#101828)}
.dshtm-action[data-danger="true"]:hover{color:var(--dsw-alias-state-error-primary,#d92d20)}
.dshtm-action:disabled{cursor:not-allowed;opacity:.4}
.dshtm-edit{display:flex;min-width:0;gap:5px}
.dshtm-input{min-width:0;flex:1;height:27px;box-sizing:border-box;border:1px solid var(--dsw-alias-border-l2,#d0d5dd);border-radius:6px;background:var(--dsw-alias-bg-base,#fff);color:inherit;padding:2px 7px;font:13px/19px Inter,sans-serif;outline:none}
.dshtm-input:focus{border-color:var(--dsw-alias-interactive-primary,#175cd3);box-shadow:0 0 0 2px color-mix(in srgb,var(--dsw-alias-interactive-primary,#175cd3) 18%,transparent)}
.dshtm-foot{display:flex;align-items:center;gap:8px;padding:9px 12px;border-top:1px solid var(--dsw-alias-border-l1,rgba(127,127,137,.14));font-size:11px;color:var(--dsw-alias-label-caption,#98a2b3)}
.dshtm-stop-turn{margin-left:auto;border:0;border-radius:7px;background:transparent;color:var(--dsw-alias-state-error-primary,#d92d20);padding:5px 8px;font-size:12px;cursor:pointer}
.dshtm-stop-turn:hover{background:color-mix(in srgb,var(--dsw-alias-state-error-primary,#d92d20) 10%,transparent)}
.dshtm-stop-turn:disabled{cursor:not-allowed;opacity:.4}
.dshtm-empty{padding:20px 16px;text-align:center;color:var(--dsw-alias-label-tertiary,#667085);font-size:13px}
.dshtm-error{padding:7px 12px;background:color-mix(in srgb,var(--dsw-alias-state-error-primary,#d92d20) 8%,transparent);color:var(--dsw-alias-state-error-primary,#d92d20);font-size:12px;line-height:17px}
@media (hover:none){.dshtm-actions{opacity:1}}
@media (max-width:520px){.dshtm-popover{position:fixed;left:12px;right:12px;bottom:86px;width:auto;max-height:60vh}}
@media (prefers-reduced-motion:reduce){.dshtm-trigger,.dshtm-actions,.dshtm-progress>span{transition:none}}
`

function installStyle() {
  if (typeof document === 'undefined' || document.querySelector(`style[data-plugin-css="${ID}/client.css"]`)) return
  const style = document.createElement('style')
  style.dataset.plugin = ID
  style.dataset.pluginCss = `${ID}/client.css`
  style.textContent = CSS
  document.head.appendChild(style)
}

function icon() {
  return createElement('svg', { viewBox: '0 0 20 20', fill: 'none', 'aria-hidden': 'true' },
    createElement('path', { d: 'M7.3 5.2h8M7.3 10h8M7.3 14.8h8M3.4 5.1l.8.8 1.5-1.7M3.4 10l.8.8 1.5-1.7M3.7 14.8h1.7', stroke: 'currentColor', strokeWidth: '1.5', strokeLinecap: 'round', strokeLinejoin: 'round' }))
}

function statusGlyph(status) {
  if (status === 'completed') return '✓'
  if (status === 'in_progress') return '◉'
  return '○'
}

function targetTodos(action, index, todos, value) {
  const target = todos.map((item) => ({ content: item.content, status: item.status }))
  if (action === 'edit') {
    target[index] = { ...target[index], content: value.trim() }
    return target
  }
  const [removed] = target.splice(index, 1)
  if (removed?.status === 'in_progress' && !target.some((item) => item.status === 'in_progress')) {
    const next = target.find((item) => item.status === 'pending')
    if (next) next.status = 'in_progress'
  }
  return target
}

function fallbackCommand(action, index, todos, value) {
  const original = todos.map((item) => ({ content: item.content, status: item.status }))
  const target = targetTodos(action, index, todos, value)
  const verb = action === 'edit' ? '修改' : '停止并移除'
  return `这是用户从任务清单界面发出的确定性变更。请立即调用 todo_write，把完整任务清单精确写成“目标清单”JSON；不要自行增删、重排或更改其他状态，不要执行其他工作，也不要只用文字确认。操作：${verb}第 ${index + 1} 项。原始清单：${JSON.stringify(original)}。目标清单：${JSON.stringify(target)}。如果当前清单已不同于原始清单，不得覆盖并发的新变化；只对内容仍匹配原第 ${index + 1} 项的任务应用本操作，并保持最新的其他项不变。`
}

function activeChangeNotice(action, item, value) {
  return action === 'edit'
    ? `用户刚刚把当前任务 ${JSON.stringify(item.content)} 修改为 ${JSON.stringify(value.trim())}。请停止按旧描述继续，立即按修改后的任务执行，并以最新任务清单为准。`
    : `用户刚刚停止并移除了当前任务 ${JSON.stringify(item.content)}。请停止继续执行该任务，并以最新任务清单为准处理剩余工作。`
}

function TaskManager({ useProjection, useSession, updateTodos, sendInstruction, cancelTurn }) {
  const todos = useProjection('todos') ?? []
  const running = useSession((state) => state.running)
  const subagent = useSession((state) => state.subagent)
  const [open, setOpen] = useState(false)
  const [pinned, setPinned] = useState(false)
  const [editing, setEditing] = useState(null)
  const [busy, setBusy] = useState(null)
  const [error, setError] = useState('')
  const rootRef = useRef(null)
  const panelId = useId()
  const done = useMemo(() => todos.filter((item) => item.status === 'completed').length, [todos])
  const active = useMemo(() => todos.filter((item) => item.status === 'in_progress').length, [todos])
  const pending = todos.length - done - active
  const mutable = subagent?.address.mode !== 'one-shot'

  useEffect(() => {
    if (!open) return
    const close = (event) => {
      if (!rootRef.current?.contains(event.target)) {
        setOpen(false)
        setPinned(false)
      }
    }
    const escape = (event) => {
      if (event.key === 'Escape') {
        if (editing) setEditing(null)
        else {
          setOpen(false)
          setPinned(false)
        }
      }
    }
    document.addEventListener('pointerdown', close)
    window.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', close)
      window.removeEventListener('keydown', escape)
    }
  }, [open, editing])

  useEffect(() => {
    if (editing && !todos.some((item) => item.content === editing.original)) setEditing(null)
  }, [todos, editing])

  const apply = async (action, index, item, value = '') => {
    setBusy(index)
    setError('')
    try {
      const next = targetTodos(action, index, todos, value)
      const direct = subagent === null && await updateTodos(todos, next)
      if (!direct) {
        await sendInstruction(fallbackCommand(action, index, todos, value), running)
      } else if (running && item.status === 'in_progress') {
        try {
          await sendInstruction(activeChangeNotice(action, item, value), true)
        } catch (reason) {
          const detail = reason instanceof Error ? reason.message : String(reason)
          setError(`任务清单已更新，但当前运行未收到变更通知：${detail}`)
        }
      }
      setEditing(null)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setBusy(null)
    }
  }

  const stopTurn = async () => {
    setBusy('turn')
    setError('')
    try { await cancelTurn() }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)) }
    finally { setBusy(null) }
  }

  const summary = todos.length === 0
    ? '暂无任务'
    : `${done}/${todos.length} 已完成${active ? ` · ${active} 进行中` : ''}${pending ? ` · ${pending} 待处理` : ''}`
  return createElement('div', {
    className: 'dshtm-root',
    ref: rootRef,
    onMouseEnter: () => setOpen(true),
    onMouseLeave: () => { if (!pinned && !editing) setOpen(false) },
  },
    createElement('button', {
      type: 'button', className: 'dshtm-trigger', title: `任务清单：${summary}`,
      'aria-label': `任务清单，${summary}`, 'aria-expanded': open, 'aria-controls': panelId,
      'data-open': open ? 'true' : 'false',
      onFocus: () => setOpen(true),
      onClick: () => {
        setPinned((value) => {
          const next = !value
          setOpen(next)
          return next
        })
      },
    }, icon(), todos.length > 0 && createElement('span', { className: 'dshtm-badge', 'aria-hidden': 'true' }, todos.length)),
    open && createElement('section', { className: 'dshtm-popover', id: panelId, 'aria-label': '任务清单管理' },
      createElement('div', { className: 'dshtm-head' },
        createElement('span', { className: 'dshtm-title' }, '任务清单'),
        createElement('span', { className: 'dshtm-summary' }, summary)),
      createElement('div', { className: 'dshtm-progress', 'aria-hidden': 'true' },
        createElement('span', { style: { width: `${todos.length ? done / todos.length * 100 : 0}%` } })),
      error && createElement('div', { className: 'dshtm-error', role: 'alert' }, error),
      todos.length === 0 ? createElement('div', { className: 'dshtm-empty' }, '暂无任务；智能体创建任务清单后会自动显示进度。') :
        createElement('ul', { className: 'dshtm-list' }, todos.map((item, index) =>
          createElement('li', { className: 'dshtm-row', 'data-status': item.status, key: `${index}:${item.content}` },
            createElement('span', { className: 'dshtm-status', title: item.status }, statusGlyph(item.status)),
            editing?.index === index ? createElement('div', { className: 'dshtm-edit' },
              createElement('input', {
                autoFocus: true, className: 'dshtm-input', value: editing.value, 'aria-label': '修改任务内容',
                onChange: (event) => setEditing({ ...editing, value: event.currentTarget.value }),
                onKeyDown: (event) => {
                  if (event.key === 'Enter' && !event.nativeEvent.isComposing && editing.value.trim()) apply('edit', index, item, editing.value)
                  if (event.key === 'Escape') setEditing(null)
                },
              }),
              createElement('button', { type: 'button', className: 'dshtm-action', title: '保存', disabled: busy !== null || !editing.value.trim(), onClick: () => apply('edit', index, item, editing.value) }, '✓'),
              createElement('button', { type: 'button', className: 'dshtm-action', title: '取消修改', disabled: busy !== null, onClick: () => setEditing(null) }, '×')) :
              createElement('span', { className: 'dshtm-content' }, item.content),
            editing?.index !== index && createElement('div', { className: 'dshtm-actions' },
              createElement('button', { type: 'button', className: 'dshtm-action', title: '修改任务', disabled: busy !== null || !mutable, onClick: () => setEditing({ index, original: item.content, value: item.content }) }, '✎'),
              item.status !== 'completed' && createElement('button', { type: 'button', className: 'dshtm-action', 'data-danger': 'true', title: '停止并移除这个任务', disabled: busy !== null || !mutable, onClick: () => apply('stop', index, item) }, '■'))))))),
      todos.length > 0 && createElement('div', { className: 'dshtm-foot' },
        createElement('span', null, '修改会同步官方清单；进行中的任务会通知智能体'),
        running && createElement('button', { type: 'button', className: 'dshtm-stop-turn', disabled: busy !== null || !mutable, onClick: stopTurn }, busy === 'turn' ? '正在停止…' : '停止当前运行'))))
}

export function apply(ctx) {
  installStyle()
  ctx.slots.inject('conversation.input.right', () => ctx.slots.register({
    name: 'conversation.input.right',
    id: 'task-manager',
    order: 80,
    label: '任务清单',
    inject: (sessionId) => {
      const actx = ctx.sessions.scope(sessionId)
      if (!actx) throw new Error(`task manager: session "${sessionId}" resolved no scope`)
      const conversation = actx.get('conversation')
      if (!conversation) throw new Error('task manager: conversation service unavailable')
      return {
        updateTodos: async (expected, todos) => {
          const rpcId = typeof globalThis.crypto?.randomUUID === 'function'
            ? globalThis.crypto.randomUUID()
            : `${Date.now()}-${Math.random().toString(16).slice(2)}`
          const message = {
            type: 'client-request',
            rpcId,
            method: 'session.updateTodos',
            payload: { sessionId, expected, todos },
          }
          const response = await fetch('/api/session.updateTodos', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(message),
          })
          if (response.status === 404) return false
          if (!response.ok) throw new Error(`更新任务清单失败（HTTP ${response.status}）`)
          const envelope = await response.json()
          if (envelope.rpcId !== rpcId) throw new Error('更新任务清单失败：响应标识不匹配')
          if (!envelope.result?.ok) throw new Error(envelope.result?.error?.message ?? '更新任务清单失败')
          return envelope.result.value?.accepted === true
        },
        sendInstruction: async (text, running) => {
          const session = ctx.sessions.binding(sessionId)?.session
          if (!session) throw new Error('当前会话不可用')
          const result = await session.prompt([{ type: 'text', text }], running ? 'steer' : 'queue')
          if (!result.ok) throw new Error(result.error.message)
        },
        cancelTurn: () => conversation.cancel(),
      }
    },
  }, TaskManager))
}
