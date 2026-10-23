import { expect, it } from 'vitest'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { InboxDialog } from '../src/components/InboxDialog'
import { EMPTY_INBOX, inboxSelector, useInbox } from '../src/lib/inbox'

// React needs this to run `act` outside a test renderer.
;(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true

it('the inbox selector always returns the same thing', () => {
  const select = inboxSelector('acc-1')
  const state = useInbox.getState()
  // A new object every call would never settle: React re-renders as long as
  // the snapshot differs from the last one, and the window stays blank.
  expect(select(state)).toBe(select(state))
  expect(select(state)).toBe(EMPTY_INBOX)
  // The same goes for an account without a box, and for no account at all.
  expect(inboxSelector(null)(state)).toBe(EMPTY_INBOX)
  expect(inboxSelector(undefined)(state)).toBe(EMPTY_INBOX)
})

it('the inbox dialog renders', async () => {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const root = createRoot(host)
  await act(async () => {
    root.render(
      createElement(InboxDialog, {
        open: false,
        onClose: () => {},
        accountId: 'acc-1',
        onOpen: () => {},
      })
    )
  })
  expect(host.innerHTML).toContain('inbox-dialog')
  await act(async () => {
    root.unmount()
  })
})
