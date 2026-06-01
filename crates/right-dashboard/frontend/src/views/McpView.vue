<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  mcpAdd,
  mcpDetect,
  mcpOAuthStatus,
  mcpRemove,
  mcpServers,
  mcpSetHeaders,
  mcpStartOAuth,
  DashboardApiError,
} from '../api'
import type {
  McpAuthMode,
  McpDetectResponse,
  McpHeaderInput,
  McpOAuthStatusResponse,
  McpServerSummary,
  McpServersResponse,
} from '../types'
import SecretInput from '../components/SecretInput.vue'
import {
  canSaveServer as canSaveServerState,
  createDetectionRequest,
  evaluateHttpUrlSubmit,
  isOAuthTerminalStatus,
  mcpStatusDetail,
  nonEmptyHeaders,
  openOAuthUrl,
  oauthPollUnavailableStatus,
  oauthStatusMessage,
  resetAddFlowState,
  seedHeaderRows,
  shouldApplyDetectionResult,
  shouldApplyOAuthPollResult,
} from './mcpViewModel'

const servers = ref<McpServersResponse | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
const addOpen = ref(false)
const name = ref('')
const url = ref('')
const detection = ref<McpDetectResponse | null>(null)
const selectedMode = ref<McpAuthMode>('headers')
const addHeaderRows = ref<McpHeaderInput[]>([{ name: 'Authorization', value: '' }])
const editingServer = ref<string | null>(null)
const serverHeaderRows = ref<Record<string, McpHeaderInput[]>>({})
const busyAction = ref<string | null>(null)
const addWarn = ref<string | null>(null)
// Set once a plaintext-http URL has been flagged; a second Save then proceeds.
const addWarnAck = ref(false)
const oauthFlows = ref<Record<string, string>>({})
const oauthStatuses = ref<Record<string, McpOAuthStatusResponse>>({})
const oauthPollTimers = new Map<string, ReturnType<typeof window.setTimeout>>()
const detectingAddAuth = ref(false)
let disposed = false
let addFormGeneration = 0
let latestDetectionRequestId = 0
let activeDetectionRequestId: number | null = null

const recommendationLabel = computed(() => modeLabel(detection.value?.recommended_mode ?? null))
const canSaveServer = computed(() => canSaveServerState({
  name: name.value,
  url: url.value,
  busyAction: busyAction.value,
  detectingAddAuth: detectingAddAuth.value,
}))

onMounted(() => {
  void refresh()
})

onBeforeUnmount(() => {
  disposed = true
  for (const timer of oauthPollTimers.values()) {
    window.clearTimeout(timer)
  }
  oauthPollTimers.clear()
})

watch(url, () => {
  detection.value = null
  selectedMode.value = 'headers'
  latestDetectionRequestId += 1
  addWarnAck.value = false
  addWarn.value = null
})

async function refresh(): Promise<void> {
  if (disposed) {
    return
  }
  loading.value = true
  error.value = null
  try {
    const response = await mcpServers()
    if (disposed) {
      return
    }
    servers.value = response
  } catch (err) {
    if (disposed) {
      return
    }
    error.value = err instanceof Error ? err.message : 'MCP unavailable'
  } finally {
    if (!disposed) {
      loading.value = false
    }
  }
}

async function detect(): Promise<void> {
  const request = createDetectionRequest({
    formGeneration: addFormGeneration,
    latestRequestId: latestDetectionRequestId,
    url: url.value,
  })
  if (!request) {
    return
  }
  latestDetectionRequestId = request.requestId
  activeDetectionRequestId = request.requestId
  detectingAddAuth.value = true
  error.value = null
  try {
    const response = await mcpDetect(request.url)
    if (shouldApplyDetectionResult(request, {
      formGeneration: addFormGeneration,
      latestRequestId: latestDetectionRequestId,
      url: url.value,
    })) {
      detection.value = response
      selectedMode.value = response.recommended_mode
    }
  } catch (err) {
    if (shouldApplyDetectionResult(request, {
      formGeneration: addFormGeneration,
      latestRequestId: latestDetectionRequestId,
      url: url.value,
    })) {
      error.value = err instanceof Error ? err.message : 'Failed to detect MCP auth'
    }
  } finally {
    if (activeDetectionRequestId === request.requestId) {
      detectingAddAuth.value = false
      activeDetectionRequestId = null
    }
  }
}

function addHeaderRow(): void {
  addHeaderRows.value = [...addHeaderRows.value, { name: '', value: '' }]
}

function removeHeaderRow(index: number): void {
  addHeaderRows.value = addHeaderRows.value.filter((_, rowIndex) => rowIndex !== index)
}

async function saveServer(): Promise<void> {
  const httpCheck = evaluateHttpUrlSubmit(url.value, addWarnAck.value)
  if (!httpCheck.proceed) {
    addWarn.value = httpCheck.warning
    addWarnAck.value = true
    return
  }
  addWarn.value = null
  busyAction.value = 'add'
  error.value = null
  try {
    await mcpAdd({
      name: name.value.trim(),
      url: selectedMode.value === 'url_as_is' ? url.value.trim() : detection.value?.bare_url ?? url.value.trim(),
      mode: selectedMode.value,
      headers: selectedMode.value === 'headers' ? nonEmptyHeaders(addHeaderRows.value) : [],
    })
    resetAdd()
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to add MCP server'
  } finally {
    busyAction.value = null
  }
}

function startHeaderEdit(server: McpServerSummary): void {
  if (server.protected) {
    return
  }
  editingServer.value = server.name
  serverHeaderRows.value = {
    ...serverHeaderRows.value,
    [server.name]: seedHeaderRows(server),
  }
}

function cancelHeaderEdit(server: McpServerSummary): void {
  if (editingServer.value === server.name) {
    editingServer.value = null
  }
  const next = { ...serverHeaderRows.value }
  delete next[server.name]
  serverHeaderRows.value = next
}

function addServerHeaderRow(serverName: string): void {
  serverHeaderRows.value = {
    ...serverHeaderRows.value,
    [serverName]: [...rowsForServer(serverName), { name: '', value: '' }],
  }
}

function removeServerHeaderRow(serverName: string, index: number): void {
  serverHeaderRows.value = {
    ...serverHeaderRows.value,
    [serverName]: rowsForServer(serverName).filter((_, rowIndex) => rowIndex !== index),
  }
}

async function replaceHeaders(server: McpServerSummary): Promise<void> {
  if (server.protected) {
    return
  }
  busyAction.value = `headers:${server.name}`
  error.value = null
  try {
    await mcpSetHeaders(server.name, nonEmptyHeaders(rowsForServer(server.name)))
    cancelHeaderEdit(server)
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to update headers'
  } finally {
    busyAction.value = null
  }
}

async function startOAuth(server: McpServerSummary): Promise<void> {
  busyAction.value = `oauth:${server.name}`
  error.value = null
  try {
    const response = await mcpStartOAuth(server.name)
    oauthFlows.value = { ...oauthFlows.value, [server.name]: response.flow_id }
    oauthStatuses.value = {
      ...oauthStatuses.value,
      [server.name]: {
        flow_id: response.flow_id,
        server_name: server.name,
        status: 'pending',
        message: null,
        updated_at: new Date().toISOString(),
      },
    }
    scheduleOAuthPoll(server.name, response.flow_id)
    try {
      openOAuthUrl(response.auth_url, {
        openTelegramLink: window.Telegram?.WebApp?.openLink?.bind(window.Telegram.WebApp),
        assignLocation: (authUrl) => {
          window.location.href = authUrl
        },
      })
    } catch (err) {
      clearOAuthFlow(server.name)
      throw err
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to start OAuth'
  } finally {
    busyAction.value = null
  }
}

function scheduleOAuthPoll(serverName: string, flowId: string): void {
  if (disposed) {
    return
  }
  clearOAuthPoll(serverName)
  const timer = window.setTimeout(() => {
    void pollOAuthStatus(serverName, flowId)
  }, 1500)
  oauthPollTimers.set(serverName, timer)
}

function clearOAuthPoll(serverName: string): void {
  const timer = oauthPollTimers.get(serverName)
  if (timer !== undefined) {
    window.clearTimeout(timer)
    oauthPollTimers.delete(serverName)
  }
}

function clearOAuthFlow(serverName: string): void {
  clearOAuthPoll(serverName)
  const nextFlows = { ...oauthFlows.value }
  delete nextFlows[serverName]
  oauthFlows.value = nextFlows
  const nextStatuses = { ...oauthStatuses.value }
  delete nextStatuses[serverName]
  oauthStatuses.value = nextStatuses
}

async function pollOAuthStatus(serverName: string, flowId: string): Promise<void> {
  try {
    const response = await mcpOAuthStatus(flowId)
    if (disposed) {
      return
    }
    if (!shouldApplyOAuthPollResult(response.flow_id, oauthFlows.value[serverName])) {
      return
    }
    oauthStatuses.value = { ...oauthStatuses.value, [serverName]: response }
    if (isOAuthTerminalStatus(response.status)) {
      clearOAuthPoll(serverName)
      await refresh()
      if (disposed) {
        return
      }
      return
    }
    scheduleOAuthPoll(serverName, flowId)
  } catch (err) {
    if (disposed) {
      return
    }
    if (!shouldApplyOAuthPollResult(flowId, oauthFlows.value[serverName])) {
      return
    }
    if (!(err instanceof DashboardApiError && err.isLocked)) {
      oauthStatuses.value = {
        ...oauthStatuses.value,
        [serverName]: oauthPollUnavailableStatus(flowId, serverName, err),
      }
      scheduleOAuthPoll(serverName, flowId)
      return
    }
    oauthStatuses.value = {
      ...oauthStatuses.value,
      [serverName]: {
        flow_id: flowId,
        server_name: serverName,
        status: 'failed',
        message: err instanceof Error ? err.message : 'OAuth status unavailable',
        updated_at: new Date().toISOString(),
      },
    }
    clearOAuthPoll(serverName)
    await refresh()
    if (disposed) {
      return
    }
  }
}

async function removeServer(server: McpServerSummary): Promise<void> {
  if (server.protected) {
    return
  }
  busyAction.value = `remove:${server.name}`
  error.value = null
  try {
    await mcpRemove(server.name)
    clearOAuthFlow(server.name)
    if (editingServer.value === server.name) {
      editingServer.value = null
    }
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to remove MCP server'
  } finally {
    busyAction.value = null
  }
}

function rowsForServer(serverName: string): McpHeaderInput[] {
  return serverHeaderRows.value[serverName] ?? [{ name: 'Authorization', value: '' }]
}

function modeLabel(mode: McpAuthMode | null): string {
  if (mode === 'oauth') {
    return 'OAuth'
  }
  if (mode === 'url_as_is') {
    return 'URL as-is'
  }
  return 'Headers'
}

function statusClass(status: string): string {
  const normalized = status.toLowerCase()
  if (normalized === 'connected' || normalized === 'ready') {
    return 'ok'
  }
  if (normalized === 'needs_auth' || normalized === 'unreachable' || normalized === 'failed') {
    return 'bad'
  }
  return 'active'
}

function resetAdd(): void {
  const reset = resetAddFlowState({
    formGeneration: addFormGeneration,
    latestRequestId: latestDetectionRequestId,
    activeDetectionRequestId,
  })
  addFormGeneration = reset.formGeneration
  latestDetectionRequestId = reset.latestRequestId
  activeDetectionRequestId = reset.activeDetectionRequestId
  detectingAddAuth.value = false
  addOpen.value = false
  name.value = ''
  url.value = ''
  detection.value = null
  selectedMode.value = 'headers'
  addHeaderRows.value = [{ name: 'Authorization', value: '' }]
  addWarn.value = null
  addWarnAck.value = false
}
</script>

<template>
  <section class="panel mcp-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">MCP</p>
        <h2>Servers</h2>
      </div>
      <button class="tool-button" type="button" @click="addOpen ? resetAdd() : addOpen = true">
        {{ addOpen ? 'Close' : 'Add' }}
      </button>
    </header>

    <section v-if="addOpen" class="mcp-section">
      <div class="form-grid">
        <label class="field">
          <span class="label">Name</span>
          <input v-model="name" class="text-input" autocomplete="off">
        </label>
        <label class="field">
          <span class="label">URL</span>
          <input v-model="url" class="text-input" autocomplete="off">
        </label>
      </div>
      <div class="button-row">
        <button class="tool-button" type="button" :disabled="!url || detectingAddAuth" @click="detect">
          {{ detectingAddAuth ? 'Detecting' : 'Detect' }}
        </button>
      </div>

      <div v-if="detection" class="notice inline">
        <strong>Recommended: {{ recommendationLabel }}</strong>
        <span>{{ detection.reason }}</span>
        <span v-if="!detection.oauth_discovered">No OAuth metadata found.</span>
      </div>

      <div v-if="detection" class="segmented mcp-segmented">
        <button class="segment-button" :class="{ active: selectedMode === 'oauth' }" type="button" @click="selectedMode = 'oauth'">OAuth</button>
        <button class="segment-button" :class="{ active: selectedMode === 'headers' }" type="button" @click="selectedMode = 'headers'">Headers</button>
        <button class="segment-button" :class="{ active: selectedMode === 'url_as_is' }" type="button" @click="selectedMode = 'url_as_is'">URL as-is</button>
      </div>

      <div v-if="selectedMode === 'headers'" class="header-editor">
        <div v-for="(header, index) in addHeaderRows" :key="index" class="header-row">
          <input v-model="header.name" class="text-input" placeholder="Header name" autocomplete="off">
          <SecretInput v-model="header.value" placeholder="Header value" />
          <button class="tool-button" type="button" @click="removeHeaderRow(index)">Remove</button>
        </div>
        <button class="tool-button compact-button" type="button" @click="addHeaderRow">Add header</button>
      </div>

      <p v-if="addWarn" class="notice inline warn">{{ addWarn }}</p>
      <div class="button-row">
        <button class="tool-button" type="button" :disabled="!canSaveServer" @click="saveServer">
          {{ busyAction === 'add' ? 'Saving' : 'Save' }}
        </button>
        <button class="tool-button" type="button" @click="resetAdd">Cancel</button>
      </div>
    </section>

    <p v-if="error" class="notice inline">{{ error }}</p>
    <p v-if="loading" class="muted-line">Loading</p>

    <div v-if="servers" class="data-list">
      <article v-for="server in servers.servers" :key="server.name" class="data-row mcp-row static">
        <div class="row-main mcp-row-main">
          <strong>{{ server.name }}</strong>
          <small>{{ server.url ?? 'built-in' }}</small>
          <small v-if="server.header_names.length">Headers: {{ server.header_names.join(', ') }}</small>
        </div>
        <div class="row-side">
          <span class="status-pill" :class="statusClass(server.status)">
            {{ server.status }}
          </span>
          <small>{{ server.tool_count }} tools</small>
          <small>{{ server.auth_type ?? 'built-in' }}</small>
          <small v-if="mcpStatusDetail(server, new Date())" class="status-detail">
            {{ mcpStatusDetail(server, new Date()) }}
          </small>
        </div>
        <div class="button-row row-actions">
          <button
            v-if="!server.protected && (server.auth_type === 'oauth' || server.status === 'needs_auth')"
            class="tool-button"
            type="button"
            :disabled="busyAction === `oauth:${server.name}`"
            @click="startOAuth(server)"
          >
            {{ busyAction === `oauth:${server.name}` ? 'Opening' : 'Authenticate' }}
          </button>
          <button
            v-if="!server.protected && editingServer !== server.name"
            class="tool-button"
            type="button"
            @click="startHeaderEdit(server)"
          >
            Headers
          </button>
          <button
            v-if="!server.protected"
            class="tool-button"
            type="button"
            :disabled="busyAction === `remove:${server.name}`"
            @click="removeServer(server)"
          >
            {{ busyAction === `remove:${server.name}` ? 'Removing' : 'Remove' }}
          </button>
        </div>
        <p
          v-if="oauthStatuses[server.name]"
          class="notice inline oauth-status"
          :class="`oauth-status-${oauthStatuses[server.name].status}`"
        >
          {{ oauthStatusMessage(oauthStatuses[server.name]) }}
        </p>

        <div v-if="editingServer === server.name" class="server-header-editor">
          <div v-for="(header, index) in rowsForServer(server.name)" :key="index" class="header-row">
            <input v-model="header.name" class="text-input" placeholder="Header name" autocomplete="off">
            <SecretInput v-model="header.value" placeholder="Header value" />
            <button class="tool-button" type="button" @click="removeServerHeaderRow(server.name, index)">Remove</button>
          </div>
          <div class="button-row">
            <button class="tool-button compact-button" type="button" @click="addServerHeaderRow(server.name)">Add header</button>
            <button
              class="tool-button"
              type="button"
              :disabled="busyAction === `headers:${server.name}`"
              @click="replaceHeaders(server)"
            >
              {{ busyAction === `headers:${server.name}` ? 'Saving' : 'Save headers' }}
            </button>
            <button class="tool-button" type="button" @click="cancelHeaderEdit(server)">Cancel</button>
          </div>
        </div>
      </article>
      <p v-if="servers.servers.length === 0" class="muted-line">No MCP servers</p>
    </div>
  </section>
</template>

<style scoped>
.mcp-panel,
.mcp-section,
.form-grid,
.header-editor,
.server-header-editor,
.button-row,
.data-list {
  display: grid;
  gap: 8px;
}

.mcp-section {
  padding: 8px 0 10px;
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-bottom: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
}

.form-grid {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.field,
.header-row {
  min-width: 0;
}

.field {
  display: grid;
  gap: 4px;
}

.label {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.75rem;
  font-weight: 700;
}

.header-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr) auto;
  gap: 6px;
  align-items: center;
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

.mcp-segmented {
  margin-bottom: 0;
}

.button-row {
  display: flex;
  min-width: 0;
  flex-wrap: wrap;
  gap: 6px;
}

.compact-button {
  width: max-content;
}

.mcp-row {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  cursor: default;
}

.mcp-row-main {
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
}

.row-actions,
.server-header-editor {
  grid-column: 1 / -1;
}

.oauth-status {
  grid-column: 1 / -1;
}

.oauth-status-succeeded {
  border-color: rgba(25, 135, 84, 0.35);
}

.oauth-status-failed,
.oauth-status-expired,
.oauth-status-unknown {
  border-color: rgba(176, 42, 55, 0.35);
}

.server-header-editor {
  padding-top: 7px;
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
}

.tool-button:disabled {
  cursor: not-allowed;
  opacity: 0.58;
}

.notice.warn {
  color: var(--tg-theme-text-color, #17212b);
  background: rgba(214, 165, 26, 0.14);
  border: 1px solid rgba(214, 165, 26, 0.4);
  border-radius: 7px;
  padding: 6px 8px;
}

@media (max-width: 680px) {
  .form-grid,
  .header-row,
  .mcp-row {
    grid-template-columns: minmax(0, 1fr);
  }

  .row-side {
    align-items: flex-start;
  }
}
</style>
