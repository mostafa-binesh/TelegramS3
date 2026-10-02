<script lang="ts">
  import { formatBytes, formatCount, formatTimestamp } from '../lib/format';
  import type { ObjectChunkDetails, ObjectDetails, ObjectEntry } from '../lib/types';

  export let open = false;
  export let target: ObjectEntry | null = null;
  export let details: ObjectDetails | null = null;
  export let busy = false;
  export let error = '';
  export let onClose: () => void = () => {};

  function locationText(location: { peer_id: string; message_id: number; document_id?: string | null }) {
    const document = location.document_id ? ` · document ${location.document_id}` : '';
    return `${location.peer_id} · message ${location.message_id}${document}`;
  }

  function chunkLabel(chunk: ObjectChunkDetails) {
    return `Chunk ${chunk.order + 1}`;
  }

  function chunkSizeSummary(value: ObjectDetails) {
    if (!value.chunks.length) return 'Empty object';
    const first = value.chunks[0].size;
    const last = value.chunks[value.chunks.length - 1].size;
    return first === last ? `${formatBytes(first)} each` : `${formatBytes(first)} first · ${formatBytes(last)} final`;
  }

  function expiryText(value?: string | null) {
    return value ? formatTimestamp(value) : 'Never';
  }
</script>

{#if open}
  <div class="backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !busy && onClose()}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="object-info-title">
      <div class="head">
        <div>
          <p class="card-label">Object information</p>
          <h2 id="object-info-title">{details?.name ?? target?.name ?? 'Object details'}</h2>
          <p class="subtitle">{details ? `${details.bucket}/${details.key}` : target?.key ?? 'Loading the stored manifest…'}</p>
        </div>
        <button class="icon-button" type="button" aria-label="Close object information" on:click={onClose} disabled={busy}>×</button>
      </div>

      {#if busy}
        <div class="loading" role="status"><span class="spinner" aria-hidden="true"></span>Loading the complete object manifest…</div>
      {:else if error}
        <div class="error-box" role="alert"><strong>Could not load object information</strong><span>{error}</span><button class="ghost" type="button" on:click={onClose}>Close</button></div>
      {:else if details}
        <div class="summary-grid">
          <div class="summary-card"><span>File size</span><strong>{formatBytes(details.size)}</strong></div>
          <div class="summary-card"><span>Chunks</span><strong>{formatCount(details.chunks.length)}</strong><small>{chunkSizeSummary(details)}</small></div>
          <div class="summary-card"><span>Content type</span><strong class="summary-value">{details.content_type || 'unknown'}</strong></div>
          <div class="summary-card"><span>Availability</span><strong class:warning={details.replica_chunk_size_mismatch}>{details.replica_accounts} copies · {details.access_accounts} access</strong><small>{details.rechunking ? 'Re-chunking in progress' : details.commit_state}</small></div>
        </div>

        <section class="detail-section">
          <div class="section-heading"><div><p class="card-label">Manifest</p><h3>Object identity and policy</h3></div><span class="state-pill" class:warning={details.rechunking}>{details.rechunking ? 'Temporarily unavailable' : details.commit_state}</span></div>
          <dl class="detail-grid">
            <div><dt>Bucket</dt><dd>{details.bucket}</dd></div>
            <div><dt>Key</dt><dd class="breakable">{details.key}</dd></div>
            <div><dt>Object ID</dt><dd class="mono breakable">{details.object_id}</dd></div>
            <div><dt>Version ID</dt><dd class="mono breakable">{details.version_id ?? 'none'}</dd></div>
            <div><dt>Modified</dt><dd>{formatTimestamp(details.last_modified)}</dd></div>
            <div><dt>Expires</dt><dd>{expiryText(details.expires_at)}</dd></div>
            <div><dt>Schema version</dt><dd>{details.schema_version}</dd></div>
            <div><dt>Shared links</dt><dd>{formatCount(details.shared_links)}</dd></div>
            <div><dt>Checksum</dt><dd class="mono breakable">{details.checksum_algorithm}: {details.etag}</dd></div>
            <div><dt>Encryption</dt><dd>{details.encryption_enabled ? `${details.encryption_format}${details.encryption_key_id ? ` · key ${details.encryption_key_id}` : ''}` : 'Disabled'}</dd></div>
          </dl>
        </section>

        <section class="detail-section">
          <div class="section-heading"><div><p class="card-label">Telegram storage</p><h3>Manifest location</h3></div></div>
          <p class="location mono">{locationText(details.telegram)}</p>
        </section>

        {#if Object.keys(details.user_metadata).length || Object.keys(details.tags).length}
          <section class="detail-section metadata-section">
            <div class="section-heading"><div><p class="card-label">Application metadata</p><h3>Metadata and tags</h3></div></div>
            {#if Object.keys(details.user_metadata).length}<div><h4>User metadata</h4><dl class="metadata-list">{#each Object.entries(details.user_metadata) as entry}<div><dt>{entry[0]}</dt><dd class="breakable">{entry[1]}</dd></div>{/each}</dl></div>{/if}
            {#if Object.keys(details.tags).length}<div><h4>Tags</h4><dl class="metadata-list">{#each Object.entries(details.tags) as entry}<div><dt>{entry[0]}</dt><dd class="breakable">{entry[1]}</dd></div>{/each}</dl></div>{/if}
          </section>
        {/if}

        <section class="detail-section chunks-section">
          <div class="section-heading"><div><p class="card-label">Chunk layout</p><h3>{formatCount(details.chunks.length)} stored chunk{details.chunks.length === 1 ? '' : 's'}</h3></div><span class="fine-print">Offsets and checksums are from the committed manifest.</span></div>
          {#if !details.chunks.length}<p class="empty">This is an empty object and has no chunks.</p>{:else}<div class="chunk-list">{#each details.chunks as chunk (chunk.order)}<article class="chunk-card"><div class="chunk-head"><strong>{chunkLabel(chunk)}</strong><span>{formatBytes(chunk.size)} · offset {formatBytes(chunk.offset)}</span></div><dl class="chunk-grid"><div><dt>Checksum</dt><dd class="mono breakable">{chunk.checksum}</dd></div><div><dt>Primary location</dt><dd class="mono breakable">{locationText(chunk.telegram)}</dd></div><div><dt>Payload source</dt><dd class="mono breakable">{chunk.source_object_id ? `${chunk.source_object_id} · chunk ${(chunk.source_chunk_order ?? 0) + 1}` : 'This object'}</dd></div><div><dt>Replicas</dt><dd>{chunk.replicas.length ? `${chunk.replicas.length} alternate location${chunk.replicas.length === 1 ? '' : 's'}` : 'None'}</dd></div></dl>{#if chunk.replicas.length}<div class="replica-list">{#each chunk.replicas as replica}<span class="replica-pill">{replica.account_id} · {replica.mode}{replica.chunk_size ? ` · ${formatBytes(replica.chunk_size)}` : ''}</span>{/each}</div>{/if}</article>{/each}</div>{/if}
        </section>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop{position:fixed;inset:0;z-index:35;display:grid;place-items:center;padding:20px;background:rgba(12,25,42,.58)}
  .modal{width:min(940px,100%);max-height:calc(100vh - 40px);overflow:auto;padding:24px;border:1px solid var(--border);border-radius:var(--radius-lg);background:var(--surface);box-shadow:0 24px 70px rgba(13,31,52,.28)}
  .head{display:flex;align-items:flex-start;justify-content:space-between;gap:18px;padding-bottom:18px;border-bottom:1px solid #e3ebf1}.head h2{margin:.25rem 0 .35rem;color:#203b57;overflow-wrap:anywhere}.subtitle{margin:0;color:var(--muted);font-size:.78rem;overflow-wrap:anywhere}.icon-button{width:36px;height:36px;padding:0;border-radius:50%;background:var(--accent-soft);color:var(--text);font-size:1.35rem}.icon-button:disabled{opacity:.5;cursor:wait}
  .summary-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-top:18px}.summary-card{display:grid;gap:4px;min-width:0;padding:14px;border:1px solid #dfe9f0;border-radius:14px;background:#fbfdff}.summary-card span,.summary-card small{color:var(--muted);font-size:.7rem}.summary-card strong{color:#203b57;font-size:1.05rem;overflow-wrap:anywhere}.summary-card strong.warning,.state-pill.warning{color:#996017}.summary-value{font-size:.82rem!important}
  .detail-section{display:grid;gap:12px;margin-top:20px;padding-top:20px;border-top:1px solid #e8eef3}.section-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:14px}.section-heading h3{margin:.25rem 0;color:#203b57}.state-pill{padding:6px 9px;border-radius:999px;background:#e5f6ed;color:#17604a;font-size:.7rem;font-weight:800;white-space:nowrap}.detail-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px 18px;margin:0}.detail-grid>div,.metadata-list>div{display:grid;gap:3px;min-width:0;padding:9px 0;border-bottom:1px solid #edf1f5}.detail-grid dt,.metadata-list dt,.chunk-grid dt{color:var(--muted);font-size:.7rem}.detail-grid dd,.metadata-list dd,.chunk-grid dd{margin:0;color:#233e58;font-size:.78rem}.breakable{overflow-wrap:anywhere}.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.7rem!important}.location{margin:0;padding:12px;border:1px solid #e0eaf1;border-radius:11px;background:#f7fbfe;color:#365872;overflow-wrap:anywhere}.metadata-section h4{margin:0 0 4px;color:#365872;font-size:.78rem}.metadata-list{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:0 18px;margin:0}.empty,.error-box{padding:12px 0;color:var(--muted)}
  .chunk-list{display:grid;gap:10px;max-height:420px;overflow:auto;padding-right:4px}.chunk-card{padding:13px;border:1px solid #dfe9f0;border-radius:13px;background:#fbfdff}.chunk-head{display:flex;align-items:center;justify-content:space-between;gap:10px}.chunk-head strong{color:#203b57}.chunk-head span{color:var(--muted);font-size:.72rem}.chunk-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:5px 18px;margin:10px 0 0}.chunk-grid>div{min-width:0}.replica-list{display:flex;flex-wrap:wrap;gap:6px;margin-top:10px}.replica-pill{padding:5px 8px;border-radius:999px;background:#edf5fb;color:#3b607b;font-size:.68rem}.loading{display:flex;align-items:center;gap:10px;min-height:150px;color:var(--muted)}.error-box{display:grid;gap:10px;color:var(--danger)}.error-box strong{color:#8f2f2f}.error-box .ghost{justify-self:start}
  @media(max-width:760px){.summary-grid{grid-template-columns:repeat(2,minmax(0,1fr))}.detail-grid,.metadata-list,.chunk-grid{grid-template-columns:1fr}.chunk-head{align-items:flex-start;flex-direction:column}}
  @media(max-width:520px){.backdrop{padding:10px}.modal{max-height:calc(100vh - 20px);padding:16px}.summary-grid{grid-template-columns:1fr 1fr}.summary-card{padding:11px}}
</style>
