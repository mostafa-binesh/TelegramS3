<script lang="ts">
  import { formatTimestamp } from '../lib/format';
  import type { ObjectEntry, SharedLink } from '../lib/types';

  export let open = false;
  export let target: ObjectEntry | null = null;
  export let links: SharedLink[] = [];
  export let busy = false;
  export let error = '';
  export let onClose: () => void = () => {};
  export let onUpdateExpiry: (id: string, seconds: number | null) => void = () => {};
  export let onRevoke: (id: string) => void = () => {};

  let confirmingId = '';
  let editingId = '';
  let customSeconds = '';
  let copiedId = '';
  let selectedId = '';

  function selectExpiry(link: SharedLink, value: string) {
    editingId = link.id;
    if (value === 'custom') {
      customSeconds = '';
      return;
    }
    editingId = '';
    onUpdateExpiry(link.id, value === 'never' ? null : Number(value));
  }

  function saveCustomExpiry(link: SharedLink) {
    const seconds = Number(String(customSeconds ?? '').trim());
    if (!Number.isInteger(seconds) || seconds < 1) return;
    editingId = '';
    onUpdateExpiry(link.id, seconds);
  }

  async function copyLink(link: SharedLink) {
    if (!link.url) return;
    try {
      await navigator.clipboard.writeText(new URL(link.url, window.location.origin).toString());
      copiedId = link.id;
      selectedId = '';
      window.setTimeout(() => { if (copiedId === link.id) copiedId = ''; }, 1600);
    } catch {
      const input = document.querySelector<HTMLInputElement>(`input[aria-label="Shared URL ${CSS.escape(link.description)}"]`);
      input?.focus();
      input?.select();
      selectedId = link.id;
      window.setTimeout(() => { if (selectedId === link.id) selectedId = ''; }, 1600);
    }
  }
</script>

{#if open && target}
  <div class="modal-backdrop shared-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !busy && onClose()}>
    <dialog open class="modal-card shared-modal" aria-modal="true" aria-labelledby="shared-links-title">
      <div class="shared-hero">
        <div class="shared-mark" aria-hidden="true"><span></span><span></span><span></span></div>
        <div class="hero-copy">
          <p class="card-label">Link library</p>
          <h2 id="shared-links-title">Shared links</h2>
          <p>{target.name} · {links.length} {links.length === 1 ? 'link' : 'links'} currently available to manage.</p>
        </div>
        <button class="icon-button" type="button" aria-label="Close shared links" title="Close" on:click={onClose} disabled={busy}>×</button>
      </div>

      {#if error}<p class="error-hint" role="alert">{error}</p>{/if}

      {#if !links.length && !busy}
        <div class="empty-links">
          <div class="empty-orbit" aria-hidden="true">↗</div>
          <strong>No shared links yet</strong>
          <p>Create a link from the file row and it will appear here with its own expiry and controls.</p>
        </div>
      {:else}
        <div class="link-list" aria-live="polite">
          {#each links as link, index (link.id)}
            <article class:expired={link.status === 'expired'} class="link-card">
              <div class="link-card-head">
                <div>
                  <span class="link-index">LINK {String(index + 1).padStart(2, '0')}</span>
                  <h3>{link.description}</h3>
                </div>
                <span class:expired-badge={link.status === 'expired'} class="status-badge">{link.status}</span>
              </div>
              <div class="link-meta">
                <span>Created {formatTimestamp(link.created_at)}</span>
                <span>{link.expires_at ? `Expires ${formatTimestamp(link.expires_at)}` : 'Never expires'}</span>
              </div>
              {#if link.url}
                <div class="url-row">
                  <input readonly value={new URL(link.url, window.location.origin).toString()} aria-label={`Shared URL ${link.description}`} on:focus={(event) => event.currentTarget.select()} />
                  <button class="copy-button" type="button" on:click={() => copyLink(link)}>{copiedId === link.id ? 'Copied' : selectedId === link.id ? 'Selected' : 'Copy'}</button>
                </div>
              {:else}
                <p class="legacy-note">This older link cannot be reconstructed because its bearer token was not stored. Revoke it here and create a new link to manage its URL.</p>
              {/if}
              <div class="link-controls">
                <label class="expiry-control">
                  <span>Change expiry</span>
                  <select aria-label={`Change expiry for ${link.description}`} on:change={(event) => selectExpiry(link, event.currentTarget.value)} disabled={busy}>
                    <option value="">Choose a lifetime…</option>
                    <option value="900">15 minutes</option>
                    <option value="3600">1 hour</option>
                    <option value="86400">1 day</option>
                    <option value="604800">7 days</option>
                    <option value="never">Never (limited by file)</option>
                    <option value="custom">Custom seconds…</option>
                  </select>
                </label>
                {#if editingId === link.id}
                  <div class="custom-expiry">
                    <input type="number" min="1" step="1" bind:value={customSeconds} aria-label={`Custom expiry seconds for ${link.description}`} placeholder="Seconds" />
                    <button class="primary small-button" type="button" on:click={() => saveCustomExpiry(link)} disabled={busy}>Save</button>
                  </div>
                {/if}
                {#if confirmingId === link.id}
                  <div class="revoke-confirm"><span>Revoke this link?</span><button class="ghost small-button" type="button" on:click={() => confirmingId = ''} disabled={busy}>Keep</button><button class="danger-button small-button" type="button" on:click={() => { confirmingId = ''; onRevoke(link.id); }} disabled={busy}>Revoke</button></div>
                {:else}
                  <button class="ghost revoke-button" type="button" on:click={() => confirmingId = link.id} disabled={busy}>Revoke link</button>
                {/if}
              </div>
            </article>
          {/each}
        </div>
      {/if}

      <div class="shared-footer"><span>Public bearer links can download this file without S3 credentials.</span><button class="ghost" type="button" on:click={onClose} disabled={busy}>Done</button></div>
    </dialog>
  </div>
{/if}

<style>
  .shared-backdrop { z-index: 22; }
  .shared-modal { width: min(760px, 100%); gap: 18px; background: linear-gradient(145deg, #ffffff 0%, #f8fbff 100%); }
  .shared-hero { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 14px; align-items: start; padding-bottom: 4px; }
  .shared-mark { position: relative; display: grid; place-items: center; width: 52px; height: 52px; border-radius: 17px; background: linear-gradient(145deg, #dff4ec, #effbf6); color: var(--ok); }
  .shared-mark span { position: absolute; width: 18px; height: 10px; border: 2px solid currentColor; border-radius: 8px; transform: rotate(-35deg); }
  .shared-mark span:nth-child(1) { transform: translate(-5px, -4px) rotate(-35deg); }
  .shared-mark span:nth-child(2) { transform: translate(5px, 4px) rotate(-35deg); }
  .shared-mark span:nth-child(3) { width: 5px; height: 5px; border: 0; background: currentColor; }
  .hero-copy h2 { margin: .2rem 0 .35rem; letter-spacing: -.045em; }
  .hero-copy p:last-child { margin: 0; color: var(--muted); line-height: 1.45; }
  .link-list { display: grid; gap: 12px; max-height: min(56vh, 560px); overflow: auto; padding: 2px 4px 2px 1px; }
  .link-card { position: relative; display: grid; gap: 10px; padding: 16px; border: 1px solid #dce7ef; border-radius: 16px; background: rgba(255,255,255,.85); box-shadow: 0 8px 22px rgba(24, 57, 88, .06); overflow: hidden; }
  .link-card::before { content: ''; position: absolute; inset: 0 auto 0 0; width: 4px; background: linear-gradient(#2bb58a, #216dba); }
  .link-card.expired::before { background: #c5cbd2; }
  .link-card-head { display: flex; justify-content: space-between; gap: 12px; align-items: start; }
  .link-card h3 { margin: 3px 0 0; font-size: 1rem; overflow-wrap: anywhere; }
  .link-index { color: var(--accent); font-size: .68rem; letter-spacing: .15em; font-weight: 800; }
  .status-badge { flex: 0 0 auto; padding: .3rem .55rem; border-radius: 999px; background: #e5f7ef; color: var(--ok); font-size: .7rem; font-weight: 800; text-transform: uppercase; letter-spacing: .08em; }
  .expired-badge { background: #eef0f3; color: var(--muted); }
  .link-meta { display: flex; flex-wrap: wrap; gap: 8px 16px; color: var(--muted); font-size: .76rem; }
  .url-row { display: flex; gap: 8px; align-items: center; }
  .url-row input { min-width: 0; font-size: .78rem; background: #f8fbfd; }
  .copy-button, .small-button { min-height: 40px; padding: .55rem .8rem; white-space: nowrap; }
  .copy-button { background: var(--text); }
  .link-controls { display: flex; flex-wrap: wrap; gap: 10px; align-items: end; padding-top: 2px; }
  .expiry-control { display: grid; gap: 5px; flex: 1 1 220px; }
  .expiry-control span { color: var(--muted); font-size: .72rem; font-weight: 800; text-transform: uppercase; letter-spacing: .08em; }
  .expiry-control select { min-height: 40px; padding-top: .55rem; padding-bottom: .55rem; }
  .custom-expiry { display: flex; gap: 7px; flex: 1 1 180px; }
  .custom-expiry input { min-width: 0; min-height: 40px; }
  .revoke-button { min-height: 40px; color: var(--danger); border-color: #edcaca; }
  .revoke-confirm { display: flex; flex-wrap: wrap; gap: 7px; align-items: center; color: var(--danger); font-size: .8rem; font-weight: 700; }
  .legacy-note { margin: 0; padding: 10px; border-radius: 10px; color: #7b5a25; background: #fff8e8; font-size: .78rem; line-height: 1.45; }
  .empty-links { display: grid; justify-items: center; gap: 7px; padding: 32px 20px; border: 1px dashed #cbdce8; border-radius: 16px; color: var(--muted); text-align: center; }
  .empty-links strong { color: var(--text); }
  .empty-links p { max-width: 42ch; margin: 0; line-height: 1.5; font-size: .86rem; }
  .empty-orbit { display: grid; place-items: center; width: 52px; height: 52px; border-radius: 50%; background: var(--accent-soft); color: var(--accent); font-size: 1.5rem; font-weight: 800; }
  .shared-footer { display: flex; justify-content: space-between; gap: 12px; align-items: center; padding-top: 2px; color: var(--muted); font-size: .76rem; }
  @media(max-width:600px) { .shared-hero { grid-template-columns: auto minmax(0,1fr); } .shared-hero > .icon-button { grid-column: 2; grid-row: 1; justify-self: end; } .url-row { align-items: stretch; flex-direction: column; } .shared-footer { align-items: stretch; flex-direction: column; } .shared-footer button { width: 100%; } }
</style>
