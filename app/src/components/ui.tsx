import { X } from 'lucide-react'
import {
  type ButtonHTMLAttributes,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'

type Variant = 'primary' | 'subtle' | 'ghost' | 'danger'

const variants: Record<Variant, string> = {
  primary: 'bg-accent text-accent-ink hover:opacity-92',
  subtle: 'bg-hover text-ink hover:bg-hover-2',
  ghost: 'text-ink-2 hover:bg-hover hover:text-ink',
  danger: 'text-danger hover:bg-hover',
}

export function Button({
  variant = 'subtle',
  className = '',
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      type="button"
      {...rest}
      className={`press inline-flex h-8 items-center justify-center gap-1.5 rounded-md px-3 text-[13px] font-medium disabled:opacity-45 ${variants[variant]} ${className}`}
    />
  )
}

export function IconButton({
  label,
  className = '',
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      {...rest}
      className={`press inline-flex size-7 shrink-0 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink disabled:opacity-40 ${className}`}
    >
      {children}
    </button>
  )
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string
  hint?: ReactNode
  children: ReactNode
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[12px] font-medium text-ink-2">{label}</span>
      {children}
      {hint && <span className="text-[12px] text-ink-3">{hint}</span>}
    </label>
  )
}

export const inputClass =
  'h-8 rounded-md bg-paper px-2.5 text-[13px] text-ink shadow-[inset_0_0_0_1px_var(--line)] placeholder:text-ink-3 focus:shadow-[inset_0_0_0_1.5px_var(--accent)] focus:outline-none'

export function Dialog({
  open,
  onClose,
  title,
  width = 460,
  children,
  testId,
}: {
  open: boolean
  onClose: () => void
  title: string
  width?: number
  children: ReactNode
  testId?: string
}) {
  const ref = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const d = ref.current
    if (!d) return
    if (open && !d.open) d.showModal()
    if (!open && d.open) d.close()
  }, [open])
  return (
    <dialog
      ref={ref}
      className="kw-dialog"
      data-testid={testId}
      onCancel={(e) => {
        e.preventDefault()
        onClose()
      }}
      onMouseDown={(e) => {
        if (e.target === ref.current) onClose()
      }}
    >
      {open && (
        <div className="kw-dialog-panel rounded-lg bg-raised shadow-pop" style={{ width }}>
          <div className="flex items-center justify-between py-3 pr-3 pl-5">
            <h2 className="text-[15px] font-semibold tracking-[-0.005em]">{title}</h2>
            <IconButton label="Close" onClick={onClose}>
              <X size={15} />
            </IconButton>
          </div>
          <div className="px-5 pb-5">{children}</div>
        </div>
      )}
    </dialog>
  )
}

export type MenuItem =
  | {
      label: string
      icon?: ReactNode
      hint?: string
      danger?: boolean
      disabled?: boolean
      onSelect: () => void
    }
  | 'separator'

/// A floating menu at a point (context menu) or under an element.
export function Menu({
  at,
  items,
  onClose,
  width = 220,
  initialActive = -1,
  instant = false,
}: {
  at: { x: number; y: number } | null
  items: MenuItem[]
  onClose: () => void
  width?: number
  /// Item highlighted on open, so Enter picks it.
  initialActive?: number
  /// No entrance animation (for menus that open from the keyboard).
  instant?: boolean
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState(at)
  const [active, setActive] = useState(-1)
  const selectable = items
    .map((it, i) => (it !== 'separator' && !it.disabled ? i : -1))
    .filter((i) => i >= 0)

  useLayoutEffect(() => {
    if (!at || !ref.current) return
    const r = ref.current.getBoundingClientRect()
    setPos({
      x: Math.min(at.x, window.innerWidth - r.width - 8),
      y: at.y + r.height > window.innerHeight - 8 ? Math.max(8, at.y - r.height) : at.y,
    })
  }, [at])

  const choose = useCallback(
    (i: number) => {
      const it = items[i]
      if (it && it !== 'separator' && !it.disabled) {
        onClose()
        it.onSelect()
      }
    },
    [items, onClose]
  )

  useEffect(() => {
    if (!at) return
    setActive(initialActive)
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((cur) => {
          const idx = selectable.indexOf(cur)
          const next = e.key === 'ArrowDown' ? idx + 1 : idx - 1
          return selectable[(next + selectable.length) % selectable.length] ?? -1
        })
      } else if (e.key === 'Enter') {
        e.preventDefault()
        setActive((cur) => {
          if (cur >= 0) choose(cur)
          return cur
        })
      }
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [at, choose, onClose])

  if (!at) return null
  return createPortal(
    <div
      ref={ref}
      role="menu"
      className={`${instant ? '' : 'kw-menu '}fixed z-50 rounded-lg bg-raised p-1 shadow-pop`}
      style={{ left: (pos ?? at).x, top: (pos ?? at).y, width }}
    >
      {items.map((it, i) =>
        it === 'separator' ? (
          <div key={`sep-${i}`} className="mx-2 my-1 h-px bg-line" />
        ) : (
          <button
            key={it.label}
            type="button"
            role="menuitem"
            disabled={it.disabled}
            onMouseEnter={() => setActive(i)}
            onClick={() => choose(i)}
            className={`flex h-8 w-full items-center gap-2.5 rounded-md px-2 text-left text-[13px] disabled:opacity-40 ${
              active === i ? 'bg-hover' : ''
            } ${it.danger ? 'text-danger' : 'text-ink'}`}
          >
            <span className="flex w-4 justify-center text-ink-3">{it.icon}</span>
            <span className="flex-1 truncate">{it.label}</span>
            {it.hint && <span className="text-[11px] text-ink-3">{it.hint}</span>}
          </button>
        )
      )}
    </div>,
    document.body
  )
}

/// Opens a Menu anchored under the clicked element.
export function useMenuAnchor() {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null)
  const openFrom = (el: HTMLElement) => {
    const r = el.getBoundingClientRect()
    setAt({ x: r.left, y: r.bottom + 4 })
  }
  const openAt = (x: number, y: number) => setAt({ x, y })
  return { at, openFrom, openAt, close: () => setAt(null) }
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded-[4px] px-1 font-sans text-[11px] text-ink-3 shadow-[inset_0_0_0_1px_var(--line)]">
      {children}
    </kbd>
  )
}

export function copyText(text: string) {
  return navigator.clipboard.writeText(text)
}
