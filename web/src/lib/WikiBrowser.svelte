<script lang="ts">
	import { getClient, type PageSummary } from './client.js';

	interface Props {
		pageType?: string;
		onSelect?: (slug: string) => void;
	}

	let { pageType, onSelect }: Props = $props();

	const client = getClient();
	let pages: PageSummary[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	$effect(() => {
		loading = true;
		client.listPages(pageType).then((result) => {
			pages = result;
			loading = false;
		}).catch((e) => {
			error = e.message;
			loading = false;
		});
	});
</script>

<div class="mp-browser">
	{#if loading}
		<div class="mp-browser__loading">Loading pages...</div>
	{:else if error}
		<div class="mp-browser__error">{error}</div>
	{:else if pages.length === 0}
		<div class="mp-browser__empty">No pages found</div>
	{:else}
		<ul class="mp-browser__list">
			{#each pages as page}
				<li class="mp-browser__item">
					<button
						class="mp-browser__link"
						onclick={() => onSelect?.(page.slug)}
					>
						<span class="mp-browser__type-badge" data-type={page.page_type}>
							{page.page_type}
						</span>
						<span class="mp-browser__title">{page.title}</span>
					</button>
					<p class="mp-browser__summary">{page.summary}</p>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.mp-browser__list {
		list-style: none;
		padding: 0;
		margin: 0;
	}
	.mp-browser__item {
		padding: var(--mp-spacing-sm, 0.5rem) 0;
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-browser__link {
		background: none;
		border: none;
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: var(--mp-spacing-sm, 0.5rem);
		padding: 0;
		font-size: inherit;
		color: var(--mp-color-link, #2563eb);
		text-align: left;
	}
	.mp-browser__link:hover {
		text-decoration: underline;
	}
	.mp-browser__title {
		font-weight: 600;
	}
	.mp-browser__summary {
		margin: 0.25rem 0 0;
		font-size: 0.875em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-browser__type-badge {
		font-size: 0.7em;
		padding: 0.1em 0.4em;
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-badge-bg, #f3f4f6);
		color: var(--mp-color-badge-text, #374151);
		text-transform: uppercase;
		font-weight: 700;
	}
	.mp-browser__loading,
	.mp-browser__error,
	.mp-browser__empty {
		padding: var(--mp-spacing-md, 1rem);
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-browser__error {
		color: var(--mp-color-error, #dc2626);
	}
</style>
