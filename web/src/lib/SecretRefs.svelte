<script lang="ts">
	import { getClient, type SecretRefView, type AccessView } from './client.js';

	interface Props {
		slug: string;
	}

	let { slug }: Props = $props();

	const client = getClient();
	let refs: SecretRefView[] = $state([]);
	let access: AccessView | null = $state(null);
	let loading = $state(true);
	let error: string | null = $state(null);
	let busy = $state(false);

	let newName = $state('');
	let newReference = $state('');

	// Spec 4 lint: a Public page must never gate a secret.
	let publicWithSecrets = $derived(
		(access as AccessView | null)?.base_visibility === 'public' && refs.length > 0
	);

	async function load() {
		loading = true;
		error = null;
		try {
			refs = await client.listSecretRefs(slug);
			access = await client.getAccess(slug);
		} catch (e: any) {
			error = e.message;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		slug;
		load();
	});

	async function addRef(e: Event) {
		e.preventDefault();
		if (!newName.trim() || !newReference.trim()) return;
		busy = true;
		error = null;
		try {
			await client.addSecretRef(slug, { name: newName.trim(), reference: newReference.trim() });
			newName = '';
			newReference = '';
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	async function removeRef(name: string) {
		busy = true;
		error = null;
		try {
			await client.removeSecretRef(slug, name);
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	const canEdit = $derived((access as AccessView | null)?.can_edit ?? false);
</script>

<section class="mp-secrets" aria-label="Secret references">
	<h3 class="mp-secrets__heading">Secret references</h3>
	<p class="mp-secrets__note">
		References are opaque backend locators (e.g. ARNs), never values. Values are resolved only via
		the audited agent tool — never shown here.
	</p>

	{#if error}
		<div class="mp-secrets__error">{error}</div>
	{/if}

	{#if publicWithSecrets}
		<div class="mp-secrets__lint" role="alert">
			⚠ This page is <strong>Public</strong> but carries secret references. A public page must not
			gate a secret — make the page Private or remove the references.
		</div>
	{/if}

	{#if loading}
		<div class="mp-secrets__loading">Loading…</div>
	{:else}
		{#if refs.length === 0}
			<p class="mp-secrets__empty">No secret references.</p>
		{:else}
			<ul class="mp-secrets__list">
				{#each refs as r}
					<li class="mp-secrets__item">
						<span class="mp-secrets__name">{r.name}</span>
						<span class="mp-secrets__ref">{r.reference}</span>
						{#if canEdit}
							<button
								class="mp-secrets__remove"
								onclick={() => removeRef(r.name)}
								disabled={busy}
								aria-label={`Remove secret reference ${r.name}`}
							>Remove</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}

		{#if canEdit}
			<form class="mp-secrets__add" onsubmit={addRef}>
				<input bind:value={newName} placeholder="name (e.g. db-password)" aria-label="Secret name" />
				<input
					bind:value={newReference}
					placeholder="reference (e.g. arn:aws:secretsmanager:…)"
					aria-label="Secret reference"
				/>
				<button type="submit" disabled={busy || !newName.trim() || !newReference.trim()}>Add</button>
			</form>
		{:else}
			<p class="mp-secrets__note">You do not have edit permission to change secret references.</p>
		{/if}
	{/if}
</section>

<style>
	.mp-secrets__heading {
		margin: 0 0 0.25rem;
		font-size: 1rem;
		color: var(--mp-color-heading, #111);
	}
	.mp-secrets__note {
		font-size: 0.8em;
		color: var(--mp-color-muted, #6b7280);
		margin: 0 0 0.75rem;
	}
	.mp-secrets__lint {
		background: #fffbeb;
		border: 1px solid #fcd34d;
		color: #92400e;
		padding: 0.5rem 0.75rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: 0.75rem;
		font-size: 0.85em;
	}
	.mp-secrets__list {
		list-style: none;
		margin: 0 0 0.75rem;
		padding: 0;
	}
	.mp-secrets__item {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.35rem 0;
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-secrets__name {
		font-weight: 600;
		font-size: 0.85em;
		min-width: 7rem;
	}
	.mp-secrets__ref {
		flex: 1;
		font-family: var(--mp-font-mono, monospace);
		font-size: 0.8em;
		color: var(--mp-color-muted, #6b7280);
		word-break: break-all;
	}
	.mp-secrets__remove {
		background: none;
		border: none;
		color: var(--mp-color-error, #dc2626);
		cursor: pointer;
		font-size: 0.8em;
	}
	.mp-secrets__add {
		display: flex;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.mp-secrets__add input {
		flex: 1;
		min-width: 8rem;
		padding: 0.4rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-secrets__add button {
		padding: 0.4rem 0.9rem;
		background: var(--mp-color-primary, #2563eb);
		color: white;
		border: none;
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
		font-weight: 600;
		font-size: 0.85em;
	}
	.mp-secrets__add button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.mp-secrets__empty {
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-secrets__loading {
		color: var(--mp-color-muted, #6b7280);
		font-size: 0.9em;
	}
	.mp-secrets__error {
		background: #fef2f2;
		color: var(--mp-color-error, #dc2626);
		padding: 0.5rem 0.75rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: 0.75rem;
		font-size: 0.85em;
	}
</style>
