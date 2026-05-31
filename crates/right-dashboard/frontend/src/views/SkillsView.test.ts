import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import SkillsView from './SkillsView.vue'
import type { SkillDetailResponse, SkillSummary } from '../types'

function skillStub(overrides: Partial<SkillSummary> = {}): SkillSummary {
  return {
    name: 'rightx-test-skill',
    group: 'learned',
    path: 'skills/rightx-test-skill.md',
    description: null,
    state: 'active',
    pinned: false,
    created_by: 'probe_writer',
    use_count: 3,
    patch_count: 1,
    created_at: null,
    last_used_at: null,
    last_patched_at: null,
    learn_cost_usd: 0.5,
    fix_cost_usd: 0.1,
    usage_cost_usd: 0.25,
    cache_read_tokens: 1000,
    cache_creation_tokens: 200,
    ...overrides,
  }
}

function selectedSkillStub(overrides: Partial<SkillSummary> = {}): SkillDetailResponse {
  return {
    agent: 'test-agent',
    skill: skillStub(overrides),
    content_preview: 'Skill content here',
    truncated: false,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(SkillsView, props as any),
  })
  return renderToString(app)
}

describe('SkillsView spend rows', () => {
  it('renders learn cost in the detail meta-grid', async () => {
    const html = await render({
      skills: null,
      selectedSkill: selectedSkillStub({ learn_cost_usd: 0.5 }),
      selectedSkillName: 'rightx-test-skill',
      loading: false,
      error: null,
    })
    expect(html).toContain('$0.50')
    expect(html).toContain('Learn')
  })

  it('renders fix cost in the detail meta-grid', async () => {
    const html = await render({
      skills: null,
      selectedSkill: selectedSkillStub({ fix_cost_usd: 0.1 }),
      selectedSkillName: 'rightx-test-skill',
      loading: false,
      error: null,
    })
    expect(html).toContain('$0.10')
    expect(html).toContain('Fix')
  })

  it('renders usage cost in the detail meta-grid', async () => {
    const html = await render({
      skills: null,
      selectedSkill: selectedSkillStub({ usage_cost_usd: 0.25 }),
      selectedSkillName: 'rightx-test-skill',
      loading: false,
      error: null,
    })
    expect(html).toContain('$0.25')
    expect(html).toContain('Usage')
  })

  it('renders cache read/write tokens in the detail meta-grid', async () => {
    const html = await render({
      skills: null,
      selectedSkill: selectedSkillStub({ cache_read_tokens: 1000, cache_creation_tokens: 200 }),
      selectedSkillName: 'rightx-test-skill',
      loading: false,
      error: null,
    })
    expect(html).toContain('1000')
    expect(html).toContain('200')
    expect(html).toContain('Cache r/w')
  })
})
