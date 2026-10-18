import { useApp } from '../lib/store'

export function Toasts() {
  const toasts = useApp((s) => s.toasts)
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[60] flex flex-col items-end gap-2" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.tone === 'error' ? 'alert' : 'status'}
          className={`kw-menu pointer-events-auto max-w-[360px] rounded-lg px-3.5 py-2.5 text-[13px] shadow-pop ${
            t.tone === 'error' ? 'bg-danger text-white' : 'bg-ink text-paper'
          }`}
        >
          {t.text}
        </div>
      ))}
    </div>
  )
}
