<script lang="ts">
	import { getClient, type PageFull } from './client.js';
	import { marked } from 'marked';

	interface Props {
		slug: string;
		onNavigate?: (slug: string) => void;
	}

	let { slug, onNavigate }: Props = $props();

	const client = getClient();
	let page: PageFull | null = $state(null);
	let loading = $state(true);
	let error: string | null = $state(null);

	$effect(() => {
		loading = true;
		error = null;
		client.getPage(slug).then((result) => {
			page = result;
			loading = false;
		}).catch((e) => {
			error = e.message;
			loading = false;
		});
	});

	function renderMarkdown(text: string): string {
		return marked.parse(text, { async: false }) as string;
	}
</script>

<div class="mp-page">
	{#if loading}
		<div class="mp-page__loading">Loading...</div>
	{:else if error}
		<div class="mp-page__error">{error}</div>
	{:else if page}
		<article class="mp-page__article">
			<header class="mp-page__header">
				<span class="mp-page__type" data-type={page.page_type}>{page.page_type}</span>
				<h1 class="mp-page__title">{page.title}</h1>
				<p class="mp-page__summary">{page.summary}</p>
			</header>

			{#if page.sections.length > 1}
				<nav class="mp-page__toc" aria-label="Table of contents">
					<h2 class="mp-page__toc-title">Contents</h2>
					<ol class="mp-page__toc-list">
						{#each page.sections as section}
							<li>
								<a href="#{section.heading.toLowerCase().replace(/\s+/g, '-')}">
									{section.heading}
								</a>
							</li>
						{/each}
					</ol>
				</nav>
			{/if}

			<div class="mp-page__content">
				{#each page.sections as section}
					<section id={section.heading.toLowerCase().replace(/\s+/g, '-')}>
						<h2>{section.heading}</h2>
						{@html renderMarkdown(section.content)}
					</section>
				{/each}
			</div>

			{#if page.links.length > 0}
				<footer class="mp-page__links">
					<h3>Related Pages</h3>
					<ul>
						{#each page.links as link}
							<li>
								<button class="mp-page__nav-link" onclick={() => onNavigate?.(link)}>
									{link}
								</button>
							</li>
						{/each}
					</ul>
				</footer>
			{/if}
		</article>
	{/if}
</div>

<style>
	.mp-page__article {
		max-width: var(--mp-content-max-width, 48rem);
	}
	.mp-page__header {
		margin-bottom: var(--mp-spacing-lg, 2rem);
		padding-bottom: var(--mp-spacing-md, 1rem);
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-page__title {
		margin: 0.25rem 0 0.5rem;
		font-size: 1.75rem;
		color: var(--mp-color-heading, #111);
	}
	.mp-page__summary {
		font-size: 1.1em;
		color: var(--mp-color-muted, #6b7280);
		margin: 0;
	}
	.mp-page__type {
		font-size: 0.7em;
		padding: 0.15em 0.5em;
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-badge-bg, #f3f4f6);
		color: var(--mp-color-badge-text, #374151);
		text-transform: uppercase;
		font-weight: 700;
	}
	.mp-page__toc {
		background: var(--mp-color-surface, #f9fafb);
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		padding: var(--mp-spacing-md, 1rem);
		margin-bottom: var(--mp-spacing-lg, 2rem);
	}
	.mp-page__toc-title {
		margin: 0 0 0.5rem;
		font-size: 0.9em;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-page__toc-list {
		margin: 0;
		padding-left: 1.25rem;
	}
	.mp-page__toc-list a {
		color: var(--mp-color-link, #2563eb);
		text-decoration: none;
	}
	.mp-page__toc-list a:hover {
		text-decoration: underline;
	}
	.mp-page__content :global(h2) {
		margin-top: var(--mp-spacing-lg, 2rem);
		color: var(--mp-color-heading, #111);
	}
	.mp-page__content :global(code) {
		background: var(--mp-color-surface, #f9fafb);
		padding: 0.1em 0.3em;
		border-radius: 3px;
		font-size: 0.9em;
	}
	.mp-page__content :global(pre) {
		background: var(--mp-color-surface, #f9fafb);
		padding: var(--mp-spacing-md, 1rem);
		border-radius: var(--mp-radius, 4px);
		overflow-x: auto;
	}
	.mp-page__links ul {
		list-style: none;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
	.mp-page__nav-link {
		background: none;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		padding: 0.25rem 0.75rem;
		cursor: pointer;
		color: var(--mp-color-link, #2563eb);
	}
	.mp-page__nav-link:hover {
		background: var(--mp-color-surface, #f9fafb);
	}
	.mp-page__loading,
	.mp-page__error {
		padding: var(--mp-spacing-md, 1rem);
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-page__error {
		color: var(--mp-color-error, #dc2626);
	}
</style>
