<script lang="ts">
	import {
		getClient,
		type AccessView,
		type PrincipalType,
		type Level,
		type BaseVisibility
	} from './client.js';

	interface Props {
		slug: string;
	}

	let { slug }: Props = $props();

	const client = getClient();
	let access: AccessView | null = $state(null);
	let loading = $state(true);
	let error: string | null = $state(null);
	let busy = $state(false);

	// New-grant form state
	let newPrincipalType: PrincipalType = $state('user');
	let newPrincipal = $state('');
	let newLevel: Level = $state('view');

	async function load() {
		loading = true;
		error = null;
		try {
			access = await client.getAccess(slug);
		} catch (e: any) {
			error = e.message;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		// Re-load whenever the slug changes.
		slug;
		load();
	});

	async function toggleVisibility() {
		if (!access) return;
		const next: BaseVisibility = access.base_visibility === 'public' ? 'private' : 'public';
		busy = true;
		error = null;
		try {
			await client.setAccess(slug, { base_visibility: next });
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	async function addGrant(e: Event) {
		e.preventDefault();
		if (!newPrincipal.trim()) return;
		busy = true;
		error = null;
		try {
			await client.addGrant(slug, {
				principal_type: newPrincipalType,
				principal: newPrincipal.trim(),
				level: newLevel
			});
			newPrincipal = '';
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	async function removeGrant(principal_type: PrincipalType, principal: string) {
		busy = true;
		error = null;
		try {
			await client.removeGrant(slug, { principal_type, principal });
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}
</script>

<section class="mp-access" aria-label="Access control">
	<h3 class="mp-access__heading">Access</h3>

	{#if error}
		<div class="mp-access__error">{error}</div>
	{/if}

	{#if loading}
		<div class="mp-access__loading">Loading access…</div>
	{:else if access}
		<div class="mp-access__row">
			<span class="mp-access__label">Owner</span>
			<span class="mp-access__value">{access.owner ?? '—'}</span>
		</div>

		<div class="mp-access__row">
			<span class="mp-access__label">Visibility</span>
			<span class="mp-access__value">
				<span class="mp-access__badge" data-visibility={access.base_visibility}>
					{access.base_visibility}
				</span>
				{#if access.can_manage}
					<button class="mp-access__toggle" onclick={toggleVisibility} disabled={busy}>
						Make {access.base_visibility === 'public' ? 'private' : 'public'}
					</button>
				{/if}
			</span>
		</div>

		<div class="mp-access__grants">
			<h4 class="mp-access__subheading">Grants</h4>
			{#if access.grants.length === 0}
				<p class="mp-access__empty">No explicit grants.</p>
			{:else}
				<ul class="mp-access__grant-list">
					{#each access.grants as g}
						<li class="mp-access__grant">
							<span class="mp-access__grant-kind">{g.principal_type}</span>
							<span class="mp-access__grant-principal">{g.principal}</span>
							<span class="mp-access__grant-level" data-level={g.level}>{g.level}</span>
							{#if access.can_manage}
								<button
									class="mp-access__remove"
									onclick={() => removeGrant(g.principal_type, g.principal)}
									disabled={busy}
									aria-label={`Remove grant for ${g.principal}`}
								>
									Remove
								</button>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}

			{#if access.can_manage}
				<form class="mp-access__add" onsubmit={addGrant}>
					<select bind:value={newPrincipalType} aria-label="Principal type">
						<option value="user">User</option>
						<option value="group">Group</option>
					</select>
					<input
						type="text"
						bind:value={newPrincipal}
						placeholder={newPrincipalType === 'user' ? 'email@example.com' : 'group-id'}
						aria-label="Principal"
					/>
					<select bind:value={newLevel} aria-label="Level">
						<option value="view">View</option>
						<option value="edit">Edit</option>
					</select>
					<button type="submit" disabled={busy || !newPrincipal.trim()}>Add</button>
				</form>
			{:else}
				<p class="mp-access__note">You do not have permission to manage grants for this page.</p>
			{/if}
		</div>
	{/if}
</section>

<style>
	.mp-access {
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		padding: var(--mp-spacing-md, 1rem);
		background: var(--mp-color-surface, #f9fafb);
	}
	.mp-access__heading {
		margin: 0 0 var(--mp-spacing-md, 1rem);
		font-size: 1rem;
		color: var(--mp-color-heading, #111);
	}
	.mp-access__subheading {
		margin: var(--mp-spacing-md, 1rem) 0 0.5rem;
		font-size: 0.85rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-access__row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 0.5rem;
	}
	.mp-access__label {
		width: 5.5rem;
		font-weight: 600;
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-access__badge {
		font-size: 0.75em;
		padding: 0.1em 0.5em;
		border-radius: var(--mp-radius, 4px);
		text-transform: uppercase;
		font-weight: 700;
		background: var(--mp-color-badge-bg, #f3f4f6);
		color: var(--mp-color-badge-text, #374151);
	}
	.mp-access__badge[data-visibility='public'] {
		background: #dcfce7;
		color: #166534;
	}
	.mp-access__badge[data-visibility='private'] {
		background: #fee2e2;
		color: #991b1b;
	}
	.mp-access__grant-list {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.mp-access__grant {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.35rem 0;
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-access__grant-kind {
		font-size: 0.7em;
		text-transform: uppercase;
		color: var(--mp-color-muted, #6b7280);
		width: 3rem;
	}
	.mp-access__grant-principal {
		flex: 1;
		font-family: var(--mp-font-mono, monospace);
		font-size: 0.85em;
	}
	.mp-access__grant-level {
		font-size: 0.7em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--mp-color-badge-text, #374151);
	}
	.mp-access__add {
		display: flex;
		gap: 0.5rem;
		margin-top: 0.75rem;
		flex-wrap: wrap;
	}
	.mp-access__add input {
		flex: 1;
		min-width: 8rem;
	}
	.mp-access__add input,
	.mp-access__add select {
		padding: 0.4rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-access__add button,
	.mp-access__toggle {
		padding: 0.4rem 0.9rem;
		background: var(--mp-color-primary, #2563eb);
		color: white;
		border: none;
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
		font-weight: 600;
		font-size: 0.85em;
	}
	.mp-access__toggle {
		background: none;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		color: var(--mp-color-link, #2563eb);
	}
	.mp-access__remove {
		background: none;
		border: none;
		color: var(--mp-color-error, #dc2626);
		cursor: pointer;
		font-size: 0.8em;
	}
	.mp-access__add button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.mp-access__empty,
	.mp-access__note {
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
		margin: 0.25rem 0;
	}
	.mp-access__loading {
		color: var(--mp-color-muted, #6b7280);
		font-size: 0.9em;
	}
	.mp-access__error {
		background: #fef2f2;
		color: var(--mp-color-error, #dc2626);
		padding: 0.5rem 0.75rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: 0.75rem;
		font-size: 0.85em;
	}
</style>
