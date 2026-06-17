<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { Ref } from 'vue'
import {
  providerList,
  providerTypes,
  providerCreate,
  providerRotate,
  providerConfigUpdate,
  providerRemove,
  providerPeers,
  providerShare,
  providerUnshare,
  providerBorrow,
} from '../api'
import type {
  ProviderView,
  ProviderProfileView,
  ProviderPeer,
} from '../types'
import SecretInput from '../components/SecretInput.vue'
import ProviderTypeList from './ProviderTypeList.vue'
import {
  validateSlug,
  validateEnvVar,
  validateUpstreamHosts,
  evaluateCredentialSubmit,
  providerCompositionClass,
  providerCompositionLabel,
  isBorrowed,
  borrowedOwnerLabel,
  shareTargetState,
  borrowCandidates,
  CREDENTIAL_HINT,
  HOSTS_MICROCOPY,
} from './providersViewModel'
import type { BorrowCandidate } from './providersViewModel'

const providers = ref<ProviderView[]>([])
const types = ref<ProviderProfileView[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
let disposed = false

// Add modal state
const addOpen = ref(false)
const addStep = ref<'choose-type' | 'fill-form'>('choose-type')
const addSelectedType = ref<ProviderProfileView | null>(null)
const addLabel = ref('')
const addCredential = ref('')
// Generic-specific fields
const addEnvVar = ref('')
const addUpstreamHosts = ref<string[]>([''])
const addUpstreamPathPrefix = ref('')
const addBusy = ref(false)
const addError = ref<string | null>(null)
const addWarn = ref<string | null>(null)
// Set once a prefixed Add credential has been flagged; a second Save then proceeds.
const addWarnAck = ref(false)

// Prefill for re-create flow
const prefillType = ref<string | null>(null)
const prefillLabel = ref<string | null>(null)

// Rotate modal state
const rotateOpen = ref(false)
const rotateProvider = ref<ProviderView | null>(null)
const rotateCredential = ref('')
const rotateBusy = ref(false)
const rotateError = ref<string | null>(null)
const rotateWarn = ref<string | null>(null)
// Set once a prefixed Rotate credential has been flagged; a second Save then proceeds.
const rotateWarnAck = ref(false)

// Edit modal state (generic only)
const editOpen = ref(false)
const editProvider = ref<ProviderView | null>(null)
const editEnvVar = ref('')
const editUpstreamHosts = ref<string[]>([''])
const editUpstreamPathPrefix = ref('')
const editBusy = ref(false)
const editError = ref<string | null>(null)

// Per-row busy tracking for delete
const busyDelete = ref<string | null>(null)

let nextHostInputKey = 1
const addUpstreamHostKeys = ref<number[]>(freshHostInputKeys(1))
const editUpstreamHostKeys = ref<number[]>(freshHostInputKeys(1))

function freshHostInputKeys(count: number): number[] {
  return Array.from({ length: Math.max(1, count) }, () => nextHostInputKey++)
}

function resetHostInputs(
  hostsRef: Ref<string[]>,
  keysRef: Ref<number[]>,
  hosts: string[] = [''],
): void {
  const nextHosts = hosts.length > 0 ? [...hosts] : ['']
  hostsRef.value = nextHosts
  keysRef.value = freshHostInputKeys(nextHosts.length)
}

function addHostInput(hostsRef: Ref<string[]>, keysRef: Ref<number[]>): void {
  hostsRef.value.push('')
  keysRef.value.push(nextHostInputKey++)
}

function removeHostInput(hostsRef: Ref<string[]>, keysRef: Ref<number[]>, index: number): void {
  if (hostsRef.value.length <= 1) {
    resetHostInputs(hostsRef, keysRef)
    return
  }
  hostsRef.value.splice(index, 1)
  keysRef.value.splice(index, 1)
}

function normalizedHosts(hosts: string[]): string[] {
  return hosts.map((host) => host.trim()).filter((host) => host.length > 0)
}

function addUpstreamHost(): void {
  addHostInput(addUpstreamHosts, addUpstreamHostKeys)
}

function removeAddUpstreamHost(index: number): void {
  removeHostInput(addUpstreamHosts, addUpstreamHostKeys, index)
}

function addEditUpstreamHost(): void {
  addHostInput(editUpstreamHosts, editUpstreamHostKeys)
}

function removeEditUpstreamHost(index: number): void {
  removeHostInput(editUpstreamHosts, editUpstreamHostKeys, index)
}

onMounted(() => {
  void refresh()
})

onBeforeUnmount(() => {
  disposed = true
})

async function refresh(): Promise<void> {
  if (disposed) return
  loading.value = true
  error.value = null
  try {
    // Peers power the secondary sharing feature only; a peer-discovery
    // failure must not blank the primary providers list, so it degrades to an
    // empty peer set rather than rejecting the whole refresh.
    const [listRes, typesRes, peersRes] = await Promise.all([
      providerList(),
      providerTypes(),
      providerPeers().catch((err) => {
        console.warn('provider peers unavailable:', err)
        return { peers: [] }
      }),
    ])
    if (disposed) return
    providers.value = listRes.providers
    types.value = typesRes.types
    peers.value = peersRes.peers
  } catch (err) {
    if (disposed) return
    error.value = err instanceof Error ? err.message : 'Providers unavailable'
  } finally {
    if (!disposed) loading.value = false
  }
}

// Add flow
function openAdd(prefType?: string | null, prefLabel?: string | null): void {
  addOpen.value = true
  addStep.value = 'choose-type'
  addSelectedType.value = null
  addLabel.value = prefLabel ?? ''
  addCredential.value = ''
  addEnvVar.value = ''
  resetHostInputs(addUpstreamHosts, addUpstreamHostKeys)
  addUpstreamPathPrefix.value = ''
  addError.value = null
  addWarn.value = null
  addWarnAck.value = false
  prefillType.value = prefType ?? null
  prefillLabel.value = prefLabel ?? null

  // Auto-select type if prefilling
  if (prefType) {
    const found = types.value.find((t) => t.type === prefType)
    if (found) {
      selectType(found)
    }
  }
}

function closeAdd(): void {
  addOpen.value = false
  addStep.value = 'choose-type'
  addSelectedType.value = null
  addLabel.value = ''
  addCredential.value = ''
  addEnvVar.value = ''
  resetHostInputs(addUpstreamHosts, addUpstreamHostKeys)
  addUpstreamPathPrefix.value = ''
  addError.value = null
  addWarn.value = null
  addWarnAck.value = false
  prefillType.value = null
  prefillLabel.value = null
}

function selectType(t: ProviderProfileView): void {
  addSelectedType.value = t
  addEnvVar.value = t.env_var
  addStep.value = 'fill-form'
}

function backToTypeSelect(): void {
  addStep.value = 'choose-type'
  addSelectedType.value = null
}

async function submitAdd(): Promise<void> {
  if (!addSelectedType.value) return
  const t = addSelectedType.value

  addError.value = null

  if (t.type === 'generic') {
    const slugErr = validateSlug(addLabel.value)
    if (slugErr) { addError.value = `Label: ${slugErr}`; return }
    const envErr = validateEnvVar(addEnvVar.value)
    if (envErr) { addError.value = `Env var: ${envErr}`; return }
    const hostsErr = validateUpstreamHosts(addUpstreamHosts.value)
    if (hostsErr) { addError.value = `Upstream hosts: ${hostsErr}`; return }
  }

  if (!addCredential.value.trim()) { addError.value = 'Credential is required'; return }

  const credCheck = evaluateCredentialSubmit(addCredential.value, addWarnAck.value)
  if (!credCheck.proceed) {
    addWarn.value = credCheck.warning
    addWarnAck.value = true
    return
  }
  addWarn.value = null

  addBusy.value = true
  try {
    await providerCreate({
      type: t.type,
      label: addLabel.value.trim() || undefined,
      credential: addCredential.value,
      generic: t.type === 'generic' ? {
        env_var: addEnvVar.value,
        upstream_hosts: normalizedHosts(addUpstreamHosts.value),
        upstream_path_prefix: addUpstreamPathPrefix.value.trim() || undefined,
      } : undefined,
    })
    closeAdd()
    await refresh()
  } catch (err) {
    addError.value = err instanceof Error ? err.message : 'Failed to add provider'
  } finally {
    addBusy.value = false
  }
}

// Rotate flow
function openRotate(provider: ProviderView): void {
  rotateProvider.value = provider
  rotateCredential.value = ''
  rotateError.value = null
  rotateWarn.value = null
  rotateWarnAck.value = false
  rotateOpen.value = true
}

function closeRotate(): void {
  rotateOpen.value = false
  rotateProvider.value = null
  rotateCredential.value = ''
  rotateError.value = null
  rotateWarn.value = null
  rotateWarnAck.value = false
}

async function submitRotate(): Promise<void> {
  if (!rotateProvider.value) return
  if (!rotateCredential.value.trim()) { rotateError.value = 'Credential is required'; return }
  const credCheck = evaluateCredentialSubmit(rotateCredential.value, rotateWarnAck.value)
  if (!credCheck.proceed) {
    rotateWarn.value = credCheck.warning
    rotateWarnAck.value = true
    return
  }
  rotateWarn.value = null
  rotateBusy.value = true
  rotateError.value = null
  try {
    await providerRotate(rotateProvider.value.name, rotateCredential.value)
    closeRotate()
    await refresh()
  } catch (err) {
    rotateError.value = err instanceof Error ? err.message : 'Failed to rotate credential'
  } finally {
    rotateBusy.value = false
  }
}

// Edit flow (generic only)
function openEdit(provider: ProviderView): void {
  if (!provider.generic) return
  editProvider.value = provider
  editEnvVar.value = provider.generic.env_var
  resetHostInputs(editUpstreamHosts, editUpstreamHostKeys, provider.generic.upstream_hosts ?? [''])
  editUpstreamPathPrefix.value = provider.generic.upstream_path_prefix ?? ''
  editError.value = null
  editOpen.value = true
}

function closeEdit(): void {
  editOpen.value = false
  editProvider.value = null
  editEnvVar.value = ''
  resetHostInputs(editUpstreamHosts, editUpstreamHostKeys)
  editUpstreamPathPrefix.value = ''
  editError.value = null
}

async function submitEdit(): Promise<void> {
  if (!editProvider.value) return
  const envErr = validateEnvVar(editEnvVar.value)
  if (envErr) { editError.value = `Env var: ${envErr}`; return }
  const hostsErr = validateUpstreamHosts(editUpstreamHosts.value)
  if (hostsErr) { editError.value = `Upstream hosts: ${hostsErr}`; return }
  editBusy.value = true
  editError.value = null
  try {
    await providerConfigUpdate(editProvider.value.name, {
      env_var: editEnvVar.value,
      upstream_hosts: normalizedHosts(editUpstreamHosts.value),
      upstream_path_prefix: editUpstreamPathPrefix.value.trim() || undefined,
    })
    closeEdit()
    await refresh()
  } catch (err) {
    editError.value = err instanceof Error ? err.message : 'Failed to update provider config'
  } finally {
    editBusy.value = false
  }
}

// Delete
async function deleteProvider(provider: ProviderView): Promise<void> {
  if (!window.confirm(`Remove provider "${provider.name}"?`)) return
  busyDelete.value = provider.name
  error.value = null
  try {
    await providerRemove(provider.name)
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to remove provider'
  } finally {
    busyDelete.value = null
  }
}

// Ghost re-create: open Add modal pre-filled with same type + label/config
function reCreate(provider: ProviderView): void {
  openAdd(provider.type, provider.label)
  if (provider.generic) {
    addEnvVar.value = provider.generic.env_var
    resetHostInputs(addUpstreamHosts, addUpstreamHostKeys, provider.generic.upstream_hosts)
    addUpstreamPathPrefix.value = provider.generic.upstream_path_prefix ?? ''
  }
}

function statusClass(provider: ProviderView): string {
  const k = provider.status.kind
  if (k === 'healthy') return 'ok'
  if (k === 'missing') return 'active'
  return 'bad'
}

function statusLabel(provider: ProviderView): string {
  const s = provider.status
  if (s.kind === 'healthy') return 'Healthy'
  if (s.kind === 'missing') return 'Missing'
  if (s.kind === 'unknown_builtin') {
    return `Unknown built-in: ${s.slug} (config migration required)`
  }
  return `Error: ${s.message}`
}

function typeLabel(provider: ProviderView): string {
  const profile = types.value.find((t) => t.type === provider.type)
  return profile?.display_name ?? provider.type
}

function isGhost(provider: ProviderView): boolean {
  return provider.status.kind === 'missing' || provider.status.kind === 'gateway_error'
}

// Peer (other-agent) state for sharing
const peers = ref<ProviderPeer[]>([])

// Share flow (owned providers only): pick a trusted peer to share with.
const shareOpen = ref(false)
const shareProvider = ref<ProviderView | null>(null)
const shareBusy = ref<string | null>(null)
const shareError = ref<string | null>(null)

// Per-row busy tracking for unshare (borrowed providers).
const busyUnshare = ref<string | null>(null)

function openShare(provider: ProviderView): void {
  shareProvider.value = provider
  shareError.value = null
  shareOpen.value = true
}
function closeShare(): void {
  shareOpen.value = false
  shareProvider.value = null
  shareError.value = null
}

interface ShareTarget {
  agent: string
  blocked: string | null
}
function shareTargets(): ShareTarget[] {
  const p = shareProvider.value
  if (!p) return []
  return peers.value.map((peer) => ({
    agent: peer.agent,
    blocked: shareTargetState(peer, p).blocked,
  }))
}

async function runShare(t: ShareTarget): Promise<void> {
  const p = shareProvider.value
  if (!p || t.blocked) return
  shareBusy.value = t.agent
  shareError.value = null
  try {
    await providerShare({ provider: p.name, dest_agent: t.agent })
    // Keep the modal open for further targets, but refresh peer state so the
    // just-shared target now reports 'already shared' and disables its button.
    await refresh()
  } catch (err) {
    shareError.value = err instanceof Error ? err.message : 'Share failed'
  } finally {
    shareBusy.value = null
  }
}

// Unshare flow (borrowed providers): drop this agent's borrowed reference.
async function unshareProvider(provider: ProviderView): Promise<void> {
  busyUnshare.value = provider.name
  error.value = null
  try {
    await providerUnshare({ provider: provider.name })
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to unshare provider'
  } finally {
    busyUnshare.value = null
  }
}

// Borrow flow (pull): attach a provider another agent shares into THIS agent.
// Same backend call as Share with owner/dest swapped; gated by the same
// both-sides trust. Busy is keyed by owner/name since a name may repeat across peers.
const borrowOpen = ref(false)
const borrowBusy = ref<string | null>(null)
const borrowError = ref<string | null>(null)

function borrowKey(c: BorrowCandidate): string {
  return `${c.owner}/${c.provider.name}`
}
function borrowList(): BorrowCandidate[] {
  return borrowCandidates(peers.value, providers.value)
}
function openBorrow(): void {
  borrowError.value = null
  borrowOpen.value = true
}
function closeBorrow(): void {
  borrowOpen.value = false
  borrowError.value = null
}
async function runBorrow(c: BorrowCandidate): Promise<void> {
  if (c.blocked) return
  borrowBusy.value = borrowKey(c)
  borrowError.value = null
  try {
    await providerBorrow({ owner_agent: c.owner, provider: c.provider.name })
    await refresh()
  } catch (err) {
    borrowError.value = err instanceof Error ? err.message : 'Borrow failed'
  } finally {
    borrowBusy.value = null
  }
}

// Editing the credential re-arms the soft prefix warning.
watch(addCredential, () => {
  addWarnAck.value = false
  addWarn.value = null
})
watch(rotateCredential, () => {
  rotateWarnAck.value = false
  rotateWarn.value = null
})
</script>

<template>
  <section class="panel providers-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Integrations</p>
        <h2>Providers</h2>
      </div>
      <div class="button-row">
        <button class="tool-button" type="button" @click="borrowOpen ? closeBorrow() : openBorrow()">
          {{ borrowOpen ? 'Close' : 'Borrow…' }}
        </button>
        <button class="tool-button" type="button" @click="addOpen ? closeAdd() : openAdd()">
          {{ addOpen ? 'Close' : '+ Add' }}
        </button>
      </div>
    </header>

    <!-- Borrow modal: pull a provider another agent shares into this one. -->
    <section v-if="borrowOpen" class="providers-section">
      <p class="muted-line">Borrow a provider shared by another agent:</p>
      <p v-if="borrowList().length === 0" class="muted-line">No providers available to borrow</p>
      <article
        v-for="c in borrowList()"
        :key="borrowKey(c)"
        class="data-row providers-row static"
      >
        <div class="row-main providers-row-main">
          <strong>{{ c.provider.name }}</strong>
          <small>from {{ c.owner }}</small>
          <small v-if="c.blocked">{{ c.blocked }}</small>
        </div>
        <div class="button-row row-actions">
          <button
            class="tool-button"
            type="button"
            :disabled="c.blocked !== null || borrowBusy === borrowKey(c)"
            @click="runBorrow(c)"
          >
            {{ borrowBusy === borrowKey(c) ? 'Working' : 'Borrow' }}
          </button>
        </div>
      </article>
      <p v-if="borrowError" class="notice inline">{{ borrowError }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" @click="closeBorrow">Close</button>
      </div>
    </section>

    <!-- Add modal -->
    <section v-if="addOpen" class="providers-section">
      <!-- Step 1: choose type -->
      <ProviderTypeList
        v-if="addStep === 'choose-type'"
        :types="types"
        @select="selectType"
      />

      <!-- Step 2: fill form -->
      <div v-if="addStep === 'fill-form' && addSelectedType" class="form-grid">
        <div class="field full-width">
          <span class="label">Type</span>
          <div class="type-badge-row">
            <span>{{ addSelectedType.display_name }}</span>
            <button class="tool-button compact-button" type="button" @click="backToTypeSelect">Change</button>
          </div>
        </div>

        <label class="field">
          <span class="label">Label{{ addSelectedType.type === 'generic' ? ' (slug, required)' : ' (optional)' }}</span>
          <input v-model="addLabel" class="text-input" autocomplete="off" placeholder="e.g. my-openai">
        </label>

        <template v-if="addSelectedType.type === 'generic'">
          <label class="field">
            <span class="label">Env var</span>
            <input v-model="addEnvVar" class="text-input" autocomplete="off" placeholder="e.g. OPENAI_API_KEY">
          </label>
          <div class="field full-width">
            <span class="label">Upstream hosts</span>
            <div class="hosts-list">
              <div
                v-for="(_, index) in addUpstreamHosts"
                :key="addUpstreamHostKeys[index]"
                class="host-row"
              >
                <input
                  :id="`add-upstream-host-${addUpstreamHostKeys[index]}`"
                  v-model="addUpstreamHosts[index]"
                  class="text-input"
                  autocomplete="off"
                  :aria-label="`Upstream host ${index + 1}`"
                  placeholder="e.g. api.openai.com"
                >
                <button
                  v-if="addUpstreamHosts.length > 1"
                  class="tool-button compact-button"
                  type="button"
                  @click="removeAddUpstreamHost(index)"
                >
                  Remove
                </button>
              </div>
            </div>
            <div class="button-row">
              <button class="tool-button compact-button" type="button" @click="addUpstreamHost">
                Add
              </button>
            </div>
            <span class="hint">{{ HOSTS_MICROCOPY }}</span>
          </div>
          <label class="field">
            <span class="label">Upstream path prefix (optional)</span>
            <input v-model="addUpstreamPathPrefix" class="text-input" autocomplete="off" placeholder="e.g. /v1">
          </label>
        </template>

        <label class="field full-width">
          <span class="label">Credential (API key)</span>
          <SecretInput v-model="addCredential" placeholder="Paste API key" />
          <span class="hint">{{ CREDENTIAL_HINT }}</span>
        </label>
      </div>

      <p v-if="addError" class="notice inline">{{ addError }}</p>
      <p v-if="addWarn" class="notice inline warn">{{ addWarn }}</p>

      <div v-if="addStep === 'fill-form'" class="button-row">
        <button class="tool-button" type="button" :disabled="addBusy" @click="submitAdd">
          {{ addBusy ? 'Saving' : 'Save' }}
        </button>
        <button class="tool-button" type="button" @click="closeAdd">Cancel</button>
      </div>
    </section>

    <!-- Rotate modal -->
    <section v-if="rotateOpen && rotateProvider" class="providers-section">
      <p class="muted-line">Rotate credential for <strong>{{ rotateProvider.name }}</strong>:</p>
      <label class="field">
        <span class="label">New credential</span>
        <SecretInput v-model="rotateCredential" placeholder="Paste new API key" />
        <span class="hint">{{ CREDENTIAL_HINT }}</span>
      </label>
      <p v-if="rotateError" class="notice inline">{{ rotateError }}</p>
      <p v-if="rotateWarn" class="notice inline warn">{{ rotateWarn }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" :disabled="rotateBusy" @click="submitRotate">
          {{ rotateBusy ? 'Saving' : 'Save' }}
        </button>
        <button class="tool-button" type="button" @click="closeRotate">Cancel</button>
      </div>
    </section>

    <!-- Edit modal (generic) -->
    <section v-if="editOpen && editProvider" class="providers-section">
      <p class="muted-line">Edit config for <strong>{{ editProvider.name }}</strong>:</p>
      <div class="form-grid">
        <label class="field">
          <span class="label">Env var</span>
          <input v-model="editEnvVar" class="text-input" autocomplete="off">
        </label>
        <div class="field full-width">
          <span class="label">Upstream hosts</span>
          <div class="hosts-list">
            <div
              v-for="(_, index) in editUpstreamHosts"
              :key="editUpstreamHostKeys[index]"
              class="host-row"
            >
              <input
                :id="`edit-upstream-host-${editUpstreamHostKeys[index]}`"
                v-model="editUpstreamHosts[index]"
                class="text-input"
                autocomplete="off"
                :aria-label="`Upstream host ${index + 1}`"
              >
              <button
                v-if="editUpstreamHosts.length > 1"
                class="tool-button compact-button"
                type="button"
                @click="removeEditUpstreamHost(index)"
              >
                Remove
              </button>
            </div>
          </div>
          <div class="button-row">
            <button class="tool-button compact-button" type="button" @click="addEditUpstreamHost">
              Add
            </button>
          </div>
          <span class="hint">{{ HOSTS_MICROCOPY }}</span>
        </div>
        <label class="field">
          <span class="label">Upstream path prefix (optional)</span>
          <input v-model="editUpstreamPathPrefix" class="text-input" autocomplete="off">
        </label>
      </div>
      <p v-if="editError" class="notice inline">{{ editError }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" :disabled="editBusy" @click="submitEdit">
          {{ editBusy ? 'Saving' : 'Save' }}
        </button>
        <button class="tool-button" type="button" @click="closeEdit">Cancel</button>
      </div>
    </section>

    <!-- Share modal -->
    <section v-if="shareOpen && shareProvider" class="providers-section">
      <p class="muted-line">Share <strong>{{ shareProvider.name }}</strong> with:</p>
      <p v-if="shareTargets().length === 0" class="muted-line">No eligible agents</p>
      <article
        v-for="t in shareTargets()"
        :key="t.agent"
        class="data-row providers-row static"
      >
        <div class="row-main providers-row-main">
          <strong>{{ t.agent }}</strong>
          <small v-if="t.blocked">{{ t.blocked }}</small>
        </div>
        <div class="button-row row-actions">
          <button
            class="tool-button"
            type="button"
            :disabled="t.blocked !== null || shareBusy === t.agent"
            @click="runShare(t)"
          >
            {{ shareBusy === t.agent ? 'Working' : 'Share' }}
          </button>
        </div>
      </article>
      <p v-if="shareError" class="notice inline">{{ shareError }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" @click="closeShare">Close</button>
      </div>
    </section>

    <p v-if="error" class="notice inline">{{ error }}</p>
    <p v-if="loading" class="muted-line">Loading</p>

    <div v-if="!loading" class="data-list">
      <article v-for="provider in providers" :key="provider.name" class="data-row providers-row static">
        <div class="row-main providers-row-main">
          <strong>{{ provider.name }}</strong>
          <small>{{ typeLabel(provider) }}</small>
          <small>{{ provider.env_var }}</small>
          <small v-if="provider.label">Label: {{ provider.label }}</small>
          <small v-if="isBorrowed(provider)" class="borrowed-label">{{ borrowedOwnerLabel(provider) }}</small>
        </div>
        <div class="row-side">
          <span class="status-pill" :class="statusClass(provider)">
            {{ statusLabel(provider) }}
          </span>
          <span class="status-pill" :class="providerCompositionClass(provider)">
            {{ providerCompositionLabel(provider) }}
          </span>
          <small>{{ provider.updated_at ? new Date(provider.updated_at).toLocaleDateString() : '—' }}</small>
        </div>
        <!-- Borrowed providers are read-only: the owner controls the credential. -->
        <div v-if="isBorrowed(provider)" class="button-row row-actions">
          <button
            class="tool-button"
            type="button"
            :disabled="busyUnshare === provider.name"
            @click="unshareProvider(provider)"
          >
            {{ busyUnshare === provider.name ? 'Working' : 'Unshare' }}
          </button>
        </div>
        <div v-else class="button-row row-actions">
          <button
            v-if="!isGhost(provider)"
            class="tool-button"
            type="button"
            @click="openRotate(provider)"
          >
            Rotate
          </button>
          <button
            v-if="provider.generic !== null && !isGhost(provider)"
            class="tool-button"
            type="button"
            @click="openEdit(provider)"
          >
            Edit
          </button>
          <button
            v-if="isGhost(provider)"
            class="tool-button"
            type="button"
            @click="reCreate(provider)"
          >
            Re-create
          </button>
          <button
            class="tool-button"
            type="button"
            :disabled="busyDelete === provider.name"
            @click="deleteProvider(provider)"
          >
            {{ busyDelete === provider.name ? 'Removing' : 'Remove' }}
          </button>
          <button
            v-if="!isGhost(provider)"
            class="tool-button"
            type="button"
            @click="openShare(provider)"
          >
            Share with…
          </button>
        </div>
      </article>
      <p v-if="providers.length === 0 && !loading" class="muted-line">No providers configured</p>
    </div>
  </section>
</template>

<style scoped>
.providers-panel,
.providers-section,
.form-grid,
.data-list,
.button-row {
  display: grid;
  gap: 8px;
}

.providers-section {
  padding: 8px 0 10px;
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-bottom: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
}

.form-grid {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.full-width {
  grid-column: 1 / -1;
}

.field {
  display: grid;
  gap: 4px;
  min-width: 0;
}

.label {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.75rem;
  font-weight: 700;
}

.hint {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
  line-height: 1.35;
}

.notice.warn {
  color: var(--tg-theme-text-color, #17212b);
  background: rgba(205, 161, 75, 0.14);
  border: 1px solid rgba(205, 161, 75, 0.4);
  border-radius: 7px;
  padding: 6px 8px;
}

.text-input {
  width: 100%;
  min-width: 0;
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
}

.button-row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.hosts-list {
  display: grid;
  gap: 6px;
}

.host-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 6px;
  align-items: start;
}

.compact-button {
  width: max-content;
}

.type-badge-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.providers-row {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  cursor: default;
}

.providers-row-main {
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
}

.borrowed-label {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-weight: 700;
}

.row-actions {
  grid-column: 1 / -1;
}

.tool-button:disabled {
  cursor: not-allowed;
  opacity: 0.58;
}

@media (max-width: 680px) {
  .form-grid,
  .providers-row,
  .host-row {
    grid-template-columns: minmax(0, 1fr);
  }

  .row-side {
    align-items: flex-start;
  }
}
</style>
