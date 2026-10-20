/// Page templates: a few built in, plus your own — any page under a top-level
/// page called "Templates". `{{date}}` and `{{weekday}}` in a template's title
/// or body become today's values.
import type { Page } from './workspace'

export type Template = {
  id: string
  /// What the template is called in the picker.
  name: string
  /// The new page's title.
  title: string
  icon: string
  description: string
  /// Built-ins carry their markdown; your own templates are read from the page.
  markdown?: string
  /// For your own templates: the page to copy.
  pageId?: string
}

export const BUILT_IN: Template[] = [
  {
    id: 'daily',
    name: 'Daily note',
    title: '{{date}}',
    icon: '📅',
    description: "Today's page. Opens it if it already exists.",
    markdown: '## Focus\n\n- [ ] \n\n## Notes\n\n## Done today\n',
  },
  {
    id: 'meeting',
    name: 'Meeting notes',
    title: 'Meeting {{date}}',
    icon: '🗓️',
    description: 'Agenda, notes, decisions and action items.',
    markdown:
      '**Date:** {{weekday}} {{date}} · **Attendees:** \n\n## Agenda\n\n1. \n\n## Notes\n\n## Decisions\n\n## Action items\n\n- [ ] \n',
  },
  {
    id: 'brief',
    name: 'Project brief',
    title: 'Project brief',
    icon: '🧭',
    description: 'Problem, goal, scope, plan and risks.',
    markdown:
      '## Problem\n\n## Goal\n\n## Scope\n\n**In:** \n\n**Out:** \n\n## Plan\n\n| Milestone | Owner | Date |\n| --- | --- | --- |\n|  |  |  |\n\n## Risks\n',
  },
  {
    id: 'weekly',
    name: 'Weekly review',
    title: 'Week of {{date}}',
    icon: '🔁',
    description: 'Wins, misses and next week.',
    markdown: "## Wins\n\n## What didn't go well\n\n## Next week\n\n- [ ] \n",
  },
]

/// Fills `{{date}}` (YYYY-MM-DD) and `{{weekday}}` for `now`.
export function fill(text: string, now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
  const weekday = now.toLocaleDateString(undefined, { weekday: 'long' })
  return text.replaceAll('{{date}}', date).replaceAll('{{weekday}}', weekday)
}

/// The top-level "Templates" page, if there is one.
export function templatesPage(live: Page[]): Page | undefined {
  return live.find((p) => !p.parent && p.title.trim().toLowerCase() === 'templates')
}

/// Your own templates: the pages under "Templates".
export function userTemplates(live: Page[]): Template[] {
  const home = templatesPage(live)
  if (!home) return []
  return live
    .filter((p) => p.parent === home.id)
    .map((p) => ({
      id: p.id,
      name: p.title || 'Untitled',
      title: p.title || 'Untitled',
      icon: p.icon,
      description: '',
      pageId: p.id,
    }))
}
