<script lang="ts">
	import { getClient, type SearchResult } from './client.js';

	interface Props {
		onSelect?: (slug: string) => void;
		placeholder?: string;
	}

	let { onSelect, placeholder = 'Search knowledge base...' }: Props = $props();

	const client = getClient();
	let query = $state('');
	let results: SearchResult[] = $state([]);
	let searching = $state(false);
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;

	function handleInput() {
		if (debounceTimer) clearTimeout(debounceTimer);
		if (query.trim().length < 2) {
			results = [];
			return;
		}
		debounceTimer = setTimeout(async () => {
			searching = true;
			try {
				results = await client.search(query);
			} catch {
				results = [];
			} finally {
				searching = false;
			}
		}, 300);
	}
</script>

<div class="mp-search">
	<div class="mp-search__input-wrap">
		<input
			type="search"
			class="mp-search__input"
			{placeholder}
			bind:value={query}
			oninput={handleInput}
		/>
		{#if searching}
			<span class="mp-search__spinner" aria-label="Searching"></span>
		{/if}
	</div>

	{#if results.length > 0}
		<ul class="mp-search__results">
			{#each results as result}
				<li class="mp-search__result">
					<button class="mp-search__result-btn" onclick={() => onSelect?.(result.slug)}>
						<span class="mp-search__result-title">{result.title}</span>
						<span class="mp-search__result-score">{(result.score * 100).toFixed(0)}%</span>
					</button>
					<p class="mp-search__result-summary">{result.summary}</p>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.mp-search {
		position: relative;
	}
	.mp-search__input-wrap {
		position: relative;
	}
	.mp-search__input {
		width: 100%;
		padding: 0.6rem 1rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		font-size: 1rem;
		font-family: inherit;
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-search__input:focus {
		outline: 2px solid var(--mp-color-primary, #2563eb);
		outline-offset: -1px;
	}
	.mp-search__spinner {
		position: absolute;
		right: 0.75rem;
		top: 50%;
		transform: translateY(-50%);
		width: 1rem;
		height: 1rem;
		border: 2px solid var(--mp-color-border, #e5e5e5);
		border-top-color: var(--mp-color-primary, #2563eb);
		border-radius: 50%;
		animation: mp-spin 0.6s linear infinite;
	}
	@keyframes mp-spin {
		to { transform: translateY(-50%) rotate(360deg); }
	}
	.mp-search__results {
		list-style: none;
		padding: 0;
		margin: 0.5rem 0 0;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		max-height: 24rem;
		overflow-y: auto;
		background: var(--mp-color-bg, #fff);
		box-shadow: 0 4px 12px rgba(0, 0, 0, 0.08);
	}
	.mp-search__result {
		padding: 0.5rem 0.75rem;
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-search__result:last-child {
		border-bottom: none;
	}
	.mp-search__result-btn {
		background: none;
		border: none;
		cursor: pointer;
		display: flex;
		justify-content: space-between;
		align-items: center;
		width: 100%;
		padding: 0;
		color: var(--mp-color-link, #2563eb);
		font-size: inherit;
		text-align: left;
	}
	.mp-search__result-btn:hover .mp-search__result-title {
		text-decoration: underline;
	}
	.mp-search__result-title {
		font-weight: 600;
	}
	.mp-search__result-score {
		font-size: 0.75em;
		color: var(--mp-color-muted, #6b7280);
		background: var(--mp-color-surface, #f9fafb);
		padding: 0.1em 0.4em;
		border-radius: 3px;
	}
	.mp-search__result-summary {
		margin: 0.25rem 0 0;
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
</style>
