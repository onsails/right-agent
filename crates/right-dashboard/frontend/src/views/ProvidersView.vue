<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  providerList,
  providerTypes,
  providerCreate,
  providerRotate,
  providerConfigUpdate,
  providerRemove,
} from '../api'
import type {
  ProviderView,
  ProviderProfileView,
  ProviderGenericBody,
} from '../types'
import SecretInput from '../components/SecretInput.vue'
import { validateSlug, validateEnvVar, evaluateCredentialSubmit, CREDENTIAL_HINT, HEADER_NAME_HINT } from './providersViewModel'

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
const addHeaderName = ref('')
const addUpstreamHost = ref('')
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
const editHeaderName = ref('')
const editUpstreamHost = ref('')
const editUpstreamPathPrefix = ref('')
const editBusy = ref(false)
const editError = ref<string | null>(null)

// Per-row busy tracking for delete
const busyDelete = ref<string | null>(null)

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
    const [listRes, typesRes] = await Promise.all([providerList(), providerTypes()])
    if (disposed) return
    providers.value = listRes.providers
    types.value = typesRes.types
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
  addHeaderName.value = ''
  addUpstreamHost.value = ''
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
  addHeaderName.value = ''
  addUpstreamHost.value = ''
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
    if (!addUpstreamHost.value.trim()) { addError.value = 'Upstream host is required'; return }
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
        header_name: addHeaderName.value.trim() || undefined,
        upstream_host: addUpstreamHost.value.trim(),
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
  editHeaderName.value = provider.generic.header_name ?? ''
  editUpstreamHost.value = provider.generic.upstream_host
  editUpstreamPathPrefix.value = provider.generic.upstream_path_prefix ?? ''
  editError.value = null
  editOpen.value = true
}

function closeEdit(): void {
  editOpen.value = false
  editProvider.value = null
  editEnvVar.value = ''
  editHeaderName.value = ''
  editUpstreamHost.value = ''
  editUpstreamPathPrefix.value = ''
  editError.value = null
}

async function submitEdit(): Promise<void> {
  if (!editProvider.value) return
  const envErr = validateEnvVar(editEnvVar.value)
  if (envErr) { editError.value = `Env var: ${envErr}`; return }
  if (!editUpstreamHost.value.trim()) { editError.value = 'Upstream host is required'; return }
  editBusy.value = true
  editError.value = null
  try {
    await providerConfigUpdate(editProvider.value.name, {
      env_var: editEnvVar.value,
      header_name: editHeaderName.value.trim() || undefined,
      upstream_host: editUpstreamHost.value.trim(),
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

// Ghost re-create: open Add modal pre-filled with same type + label
function reCreate(provider: ProviderView): void {
  openAdd(provider.type, provider.label)
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
        <p class="eyebrow">AI Providers</p>
        <h2>Providers</h2>
      </div>
      <button class="tool-button" type="button" @click="addOpen ? closeAdd() : openAdd()">
        {{ addOpen ? 'Close' : '+ Add' }}
      </button>
    </header>

    <!-- Add modal -->
    <section v-if="addOpen" class="providers-section">
      <!-- Step 1: choose type -->
      <div v-if="addStep === 'choose-type'" class="type-grid">
        <p class="muted-line">Choose a provider type:</p>
        <article
          v-for="t in types"
          :key="t.type"
          class="type-card"
          @click="selectType(t)"
        >
          <strong>{{ t.display_name }}</strong>
          <small>{{ t.category }}</small>
          <small>{{ t.env_var }}</small>
        </article>
        <p v-if="types.length === 0" class="muted-line">No provider types available</p>
      </div>

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
          <label class="field">
            <span class="label">Upstream host</span>
            <input v-model="addUpstreamHost" class="text-input" autocomplete="off" placeholder="e.g. api.openai.com">
          </label>
          <label class="field">
            <span class="label">Header name (optional)</span>
            <input v-model="addHeaderName" class="text-input" autocomplete="off" placeholder="e.g. Authorization">
            <span class="hint">{{ HEADER_NAME_HINT }}</span>
          </label>
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
        <label class="field">
          <span class="label">Upstream host</span>
          <input v-model="editUpstreamHost" class="text-input" autocomplete="off">
        </label>
        <label class="field">
          <span class="label">Header name (optional)</span>
          <input v-model="editHeaderName" class="text-input" autocomplete="off">
          <span class="hint">{{ HEADER_NAME_HINT }}</span>
        </label>
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

    <p v-if="error" class="notice inline">{{ error }}</p>
    <p v-if="loading" class="muted-line">Loading</p>

    <div v-if="!loading" class="data-list">
      <article v-for="provider in providers" :key="provider.name" class="data-row providers-row static">
        <div class="row-main providers-row-main">
          <strong>{{ provider.name }}</strong>
          <small>{{ typeLabel(provider) }}</small>
          <small>{{ provider.env_var }}</small>
          <small v-if="provider.label">Label: {{ provider.label }}</small>
        </div>
        <div class="row-side">
          <span class="status-pill" :class="statusClass(provider)">
            {{ statusLabel(provider) }}
          </span>
          <small>{{ provider.updated_at ? new Date(provider.updated_at).toLocaleDateString() : '—' }}</small>
        </div>
        <div class="button-row row-actions">
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
.type-grid,
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
  background: rgba(214, 165, 26, 0.14);
  border: 1px solid rgba(214, 165, 26, 0.4);
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

.compact-button {
  width: max-content;
}

.type-grid {
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
}

.type-card {
  display: grid;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  cursor: pointer;
}

.type-card:hover {
  border-color: var(--tg-theme-button_color, #2481cc);
}

.type-card strong {
  font-size: 0.84rem;
}

.type-card small {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
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

.row-actions {
  grid-column: 1 / -1;
}

.tool-button:disabled {
  cursor: not-allowed;
  opacity: 0.58;
}

@media (max-width: 680px) {
  .form-grid,
  .providers-row {
    grid-template-columns: minmax(0, 1fr);
  }

  .row-side {
    align-items: flex-start;
  }
}
</style>
