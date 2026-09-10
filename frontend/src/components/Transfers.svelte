<script lang="ts">
  import {onMount} from 'svelte';
  import {listJobs,jobAction} from '../lib/api';
  import LoadError from './LoadError.svelte';
  import ActionIcon from './ActionIcon.svelte';
  import type {TransferJob} from '../lib/types';
  export let csrf:string|null|undefined;
  export let recoveryOnly=false;
  let jobs:TransferJob[]=[];let error='';let loading=true;let refreshing=false;let offset=0;let hasMore=false;let pending=false;
  async function refresh(){refreshing=true;try{const res=await listJobs(offset);jobs=res.jobs;hasMore=res.next_offset!==null;error='';}catch(e){error=e instanceof Error?e.message:'Unable to load jobs';}finally{loading=false;refreshing=false;}}
  async function action(id:string,action:'retry'|'cancel'){pending=true;try{await jobAction(id,action,csrf);await refresh();}catch(e){error=e instanceof Error?e.message:'Action failed';}finally{pending=false;}}
  onMount(()=>{let disposed=false;let timer:ReturnType<typeof setTimeout>;const poll=async()=>{if(!document.hidden)await refresh();if(!disposed)timer=setTimeout(poll,error?10000:2000);};void poll();return()=>{disposed=true;clearTimeout(timer);};});
  // A rejected/aborted reception is not a user-visible transfer: it never
  // reached the durable queue and showing a 0/0 "completed" row is misleading.
  // Directory markers are persisted as zero-byte S3 objects, but are not user
  // transfers and should not create noise in this activity view.
  $: visible=jobs.filter(j=>(!j.key.endsWith('/') && !['receiving','reception_failed'].includes(j.state) && !(j.state==='completed' && j.chunks_total===0 && j.bytes===0)) && (!recoveryOnly||['recovery_required','cancelled','retry_wait'].includes(j.state)));
  function displayState(state:string){return state==='uploading'?'uploading to telegram':state.replaceAll('_',' ');}
</script>
<section class="card surface">
  <div class="section-head"><div><p class="card-label">{recoveryOnly?'Recovery':'Background transfers'}</p><h2>{recoveryOnly?'Resolve interrupted work':'Transfer activity'}</h2></div><button class="ghost" on:click={refresh} disabled={refreshing}>{#if refreshing}<span class="spinner" aria-hidden="true"></span>{/if}Refresh</button></div>
  <p class="fine-print">{recoveryOnly?'Staged files are retained while the worker checks ambiguous Telegram sends; only a verified byte match is repaired automatically.':'Files appear in Buckets after Telegram upload and commit finish. Transient Telegram failures retry in the background.'}</p>
  {#if error}<LoadError title="Could not load transfer activity" message={error} onRetry={refresh} />{/if}
  {#if loading}<div class="skeleton-stack"><div class="skeleton" style="height:48px"></div><div class="skeleton" style="height:48px"></div><div class="skeleton" style="height:48px"></div></div>{:else if !visible.length && !error}<p class="empty">{recoveryOnly?'No transfers need attention on this page.':'No transfers yet. Upload a file from Buckets to get started.'}</p>{:else if visible.length}
  <div class="table-scroll"><table><thead><tr><th>Object</th><th>Progress</th><th>Status</th><th>Actions</th></tr></thead><tbody>
    {#each visible as job (job.id)}<tr><td><strong>{job.key}</strong><small>{job.bucket} · {new Date(job.created_at*1000).toLocaleString()}</small><small>{job.id}</small></td>
      <td><progress max={Math.max(job.chunks_total,1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {(job.bytes/1048576).toFixed(1)} MiB staged</small></td>
      <td><span class="state" class:done={['completed','cleaned'].includes(job.state)}>{job.state==='cleaned'?'completed':displayState(job.state)}</span>{#if job.error}<small class="error-hint">{job.error}</small>{/if}{#if job.state==='retry_wait'}<small>Retry {new Date(job.next_retry*1000).toLocaleTimeString()} · attempt {job.attempts}</small>{/if}</td>
      <td class="row-actions">{#if ['retry_wait','recovery_required'].includes(job.state)&&job.operation_id}<ActionIcon name="retry" label={`Retry ${job.key}`} on:click={()=>action(job.id,'retry')} disabled={pending}/>{/if}{#if ['queued','retry_wait','recovery_required'].includes(job.state)}<ActionIcon name="cancel" label={`Cancel ${job.key}`} tone="danger" on:click={()=>action(job.id,'cancel')} disabled={pending}/>{/if}</td></tr>{/each}
  </tbody></table></div>{/if}
  <div class="pagination"><button class="ghost" disabled={offset===0} on:click={()=>{offset=Math.max(0,offset-50);void refresh();}}>Previous</button><span>Page {offset/50+1}</span><button class="ghost" disabled={!hasMore} on:click={()=>{offset+=50;void refresh();}}>Next</button></div>
</section>
<style>small{display:block;font-size:11px;color:var(--muted);margin-top:7px;overflow-wrap:anywhere}table{width:100%;min-width:620px;border-collapse:collapse;font-size:13px}th{text-align:left;font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.08em}td,th{padding:18px 12px;border-bottom:1px solid var(--border);vertical-align:top}td:first-child{max-width:300px}.state{display:inline-block;padding:5px 9px;background:#fff1d5;border-radius:5px;font-size:12px}.state.done{background:#dcf5e9;color:#17604a}progress{max-width:160px;accent-color:var(--accent)}.table-scroll{max-width:100%;overflow:auto;overscroll-behavior-inline:contain;-webkit-overflow-scrolling:touch}.empty{padding:32px 0;color:var(--muted)}.pagination{display:flex;gap:16px;align-items:center;justify-content:flex-end;margin-top:20px;font-size:12px}@media(max-width:480px){.pagination{justify-content:space-between;gap:8px}.pagination button{padding-right:.7rem;padding-left:.7rem}}</style>
