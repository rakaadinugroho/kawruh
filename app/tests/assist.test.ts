import { expect, it } from 'vitest'
import { draftPrompt, lineDiff, rewritePrompt, stripFences } from '../src/lib/assist'

it('the rewrite prompt carries the selection, the page and the ask', () => {
  const prompt = rewritePrompt('Make it Punchier', 'hello **world**', '# Page\n\nbody')
  expect(prompt.startsWith('You are'), 'CLI flag-safety: never begin with a dash').toBe(true)
  expect(prompt).toContain('hello **world**')
  expect(prompt).toContain('<<<KAWRUH_PAGE>>>')
  expect(prompt).toContain('Make it Punchier')
})

it('a long page is capped in the prompt', () => {
  const prompt = rewritePrompt('t', 'secret selection', 'x'.repeat(20_000))
  expect(prompt.length).toBeLessThan(16_000)
  expect(prompt).toContain('secret selection')
  expect(prompt).toContain('page cut off')
})

it('the draft prompt places its insert point', () => {
  const prompt = draftPrompt('A plan for tomorrow', '## Done today\n\n- shipped it', '# Page\n\nhi')
  expect(prompt).toContain('<<<INSERT_AFTER>>>')
  expect(prompt).toContain('- shipped it')
  expect(prompt).toContain('A plan for tomorrow')
})

it('a wrapped answer loses its fence, content keeps its own', () => {
  expect(stripFences('```markdown\n* a\n* b\n```')).toBe('* a\n* b')
  expect(stripFences('```\n* a\n* b\n```')).toBe('* a\n* b')
  expect(stripFences('```md\njust text\n```')).toBe('just text')
  // A language fence is actual content — a diagram, say — not an artifact.
  const mermaid = '```mermaid\ngraph TD\n  A --> B\n```'
  expect(stripFences(mermaid)).toBe(mermaid)
  expect(stripFences('**plain markdown**')).toBe('**plain markdown**')
})

it('the diff marks which lines would come and which would go', () => {
  expect(lineDiff('a\nb\nc', 'a\nx\nc')).toEqual([
    { kind: 'same', text: 'a' },
    { kind: 'del', text: 'b' },
    { kind: 'add', text: 'x' },
    { kind: 'same', text: 'c' },
  ])
  expect(lineDiff('unchanged', 'unchanged')).toEqual([{ kind: 'same', text: 'unchanged' }])
  expect(lineDiff('old', 'brand new')).toEqual([
    { kind: 'del', text: 'old' },
    { kind: 'add', text: 'brand new' },
  ])
})
