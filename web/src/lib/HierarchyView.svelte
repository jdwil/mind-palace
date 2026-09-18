<script lang="ts">
	import { getClient, type TreeNode, type PageSummary } from './client.js';

	interface Props {
		slug: string;
		onNavigate?: (slug: string) => void;
	}

	let { slug, onNavigate }: Props = $props();

	const client = getClient();
	let tree: TreeNode[] = $state([]);
	let allPages: PageSummary[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let busy = $state(false);
	let parentChoice = $state('');

	async function load() {
		loading = true;
		error = null;
		try {
			// Containment subtree for this page + the page list for the parent picker.
			[tree, allPages] = await Promise.all([client.getTree(slug), client.listPages()]);
		} catch (e: any) {
			error = e.message;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		slug;
		parentChoice = '';
		load();
	});

	async function setParent(e: Event) {
		e.preventDefault();
		busy = true;
		error = null;
		try {
			// Empty selection detaches to root; a slug reparents (server enforces
			// can_edit on both + cycle rejection).
			await client.setParent(slug, parentChoice || null);
			await load();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	// Candidate parents: any page other than this one.
	let candidates = $derived(allPages.filter((p) => p.slug !== slug));
</script>

<section class="mp-hierarchy" aria-label="Containment hierarchy">
	<h3 class="mp-hierarchy__heading">Hierarchy</h3>
	<p class="mp-hierarchy__note">
		Containment (parent → child) controls access inheritance and is distinct from Related links.
	</p>

	{#if error}
		<div class="mp-hierarchy__error">{error}</div>
	{/if}

	{#if loading}
		<div class="mp-hierarchy__loading">Loading…</div>
	{:else}
		<form class="mp-hierarchy__picker" onsubmit={setParent}>
			<label for="mp-parent-select">Parent</label>
			<select id="mp-parent-select" bind:value={parentChoice}>
				<option value="">— none (root) —</option>
				{#each candidates as p}
					<option value={p.slug}>{p.title} ({p.slug})</option>
				{/each}
			</select>
			<button type="submit" disabled={busy}>Set parent</button>
		</form>

		<div class="mp-hierarchy__tree">
			<h4 class="mp-hierarchy__subheading">Containment subtree</h4>
			{#if tree.length === 0}
				<p class="mp-hierarchy__empty">No containment children.</p>
			{:else}
				<ul class="mp-hierarchy__tree-list">
					{#each tree as node}
						<li>
							<span class="mp-hierarchy__edge" data-kind={node.edge_kind}>{node.edge_kind}</span>
							<button class="mp-hierarchy__link" onclick={() => onNavigate?.(node.slug)}>
								{node.title}
							</button>
							<span class="mp-hierarchy__type">{node.page_type}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</section>

<style>
	.mp-hierarchy__heading {
		margin: 0 0 0.25rem;
		font-size: 1rem;
		color: var(--mp-color-heading, #111);
	}
	.mp-hierarchy__subheading {
		margin: var(--mp-spacing-md, 1rem) 0 0.5rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-hierarchy__note {
		font-size: 0.8em;
		color: var(--mp-color-muted, #6b7280);
		margin: 0 0 0.75rem;
	}
	.mp-hierarchy__picker {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.mp-hierarchy__picker label {
		font-weight: 600;
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-hierarchy__picker select {
		flex: 1;
		min-width: 12rem;
		padding: 0.4rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-hierarchy__picker button {
		padding: 0.4rem 0.9rem;
		background: var(--mp-color-primary, #2563eb);
		color: white;
		border: none;
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
		font-weight: 600;
		font-size: 0.85em;
	}
	.mp-hierarchy__picker button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.mp-hierarchy__tree-list {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.mp-hierarchy__tree-list li {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.3rem 0;
	}
	.mp-hierarchy__edge {
		font-size: 0.65em;
		text-transform: uppercase;
		font-weight: 700;
		padding: 0.1em 0.4em;
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-badge-bg, #f3f4f6);
		color: var(--mp-color-badge-text, #374151);
	}
	.mp-hierarchy__link {
		background: none;
		border: none;
		color: var(--mp-color-link, #2563eb);
		cursor: pointer;
		padding: 0;
		font-size: 0.9em;
	}
	.mp-hierarchy__type {
		font-size: 0.7em;
		color: var(--mp-color-muted, #6b7280);
		text-transform: uppercase;
	}
	.mp-hierarchy__empty {
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-hierarchy__loading {
		color: var(--mp-color-muted, #6b7280);
		font-size: 0.9em;
	}
	.mp-hierarchy__error {
		background: #fef2f2;
		color: var(--mp-color-error, #dc2626);
		padding: 0.5rem 0.75rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: 0.75rem;
		font-size: 0.85em;
	}
</style>
