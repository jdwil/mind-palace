<script lang="ts">
	import { getClient } from './client.js';

	interface Section {
		heading: string;
		content: string;
	}

	interface Props {
		/** If provided, loads existing page for editing. Otherwise create mode. */
		slug?: string;
		onSave?: (slug: string) => void;
		onCancel?: () => void;
	}

	let { slug, onSave, onCancel }: Props = $props();

	const client = getClient();
	let title = $state('');
	let editSlug = $state('');
	let summary = $state('');
	let pageType = $state('Concept');
	let sections: Section[] = $state([{ heading: '', content: '' }]);
	let saving = $state(false);
	let error: string | null = $state(null);
	let isEditMode = $derived(!!slug);

	$effect(() => {
		if (slug) {
			client.getPage(slug).then((page) => {
				title = page.title;
				editSlug = page.slug;
				summary = page.summary;
				pageType = page.page_type;
				sections = page.sections.length > 0 ? page.sections : [{ heading: '', content: '' }];
			}).catch((e) => { error = e.message; });
		}
	});

	function addSection() {
		sections = [...sections, { heading: '', content: '' }];
	}

	function removeSection(index: number) {
		sections = sections.filter((_, i) => i !== index);
	}

	async function handleSubmit() {
		saving = true;
		error = null;
		try {
			if (isEditMode) {
				await client.updatePage(slug!, { title, summary, sections });
				onSave?.(slug!);
			} else {
				const created = await client.createPage({
					title,
					slug: editSlug,
					summary,
					sections,
					page_type: pageType,
				});
				onSave?.(created.slug);
			}
		} catch (e: any) {
			error = e.message;
		} finally {
			saving = false;
		}
	}
</script>

<form class="mp-editor" onsubmit|preventDefault={handleSubmit}>
	<h2 class="mp-editor__heading">{isEditMode ? 'Edit Page' : 'Create Page'}</h2>

	{#if error}
		<div class="mp-editor__error">{error}</div>
	{/if}

	<div class="mp-editor__field">
		<label for="mp-title">Title</label>
		<input id="mp-title" type="text" bind:value={title} required />
	</div>

	{#if !isEditMode}
		<div class="mp-editor__field">
			<label for="mp-slug">Slug</label>
			<input id="mp-slug" type="text" bind:value={editSlug} required pattern="[a-z0-9\-]+" />
		</div>
		<div class="mp-editor__field">
			<label for="mp-type">Type</label>
			<select id="mp-type" bind:value={pageType}>
				<option>Index</option>
				<option>Concept</option>
				<option>Entity</option>
				<option>Decision</option>
				<option>Leaf</option>
			</select>
		</div>
	{/if}

	<div class="mp-editor__field">
		<label for="mp-summary">Summary</label>
		<textarea id="mp-summary" bind:value={summary} rows="2" required></textarea>
	</div>

	<fieldset class="mp-editor__sections">
		<legend>Sections</legend>
		{#each sections as section, i}
			<div class="mp-editor__section">
				<input
					type="text"
					placeholder="Section heading"
					bind:value={section.heading}
					required
				/>
				<textarea
					placeholder="Markdown content..."
					bind:value={section.content}
					rows="6"
					required
				></textarea>
				{#if sections.length > 1}
					<button type="button" class="mp-editor__remove" onclick={() => removeSection(i)}>
						Remove
					</button>
				{/if}
			</div>
		{/each}
		<button type="button" class="mp-editor__add-section" onclick={addSection}>
			+ Add Section
		</button>
	</fieldset>

	<div class="mp-editor__actions">
		<button type="submit" class="mp-editor__save" disabled={saving}>
			{saving ? 'Saving...' : 'Save'}
		</button>
		{#if onCancel}
			<button type="button" class="mp-editor__cancel" onclick={onCancel}>Cancel</button>
		{/if}
	</div>
</form>

<style>
	.mp-editor {
		max-width: var(--mp-content-max-width, 48rem);
	}
	.mp-editor__heading {
		margin: 0 0 var(--mp-spacing-md, 1rem);
		color: var(--mp-color-heading, #111);
	}
	.mp-editor__field {
		margin-bottom: var(--mp-spacing-md, 1rem);
	}
	.mp-editor__field label {
		display: block;
		font-weight: 600;
		margin-bottom: 0.25rem;
		font-size: 0.875em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-editor__field input,
	.mp-editor__field textarea,
	.mp-editor__field select {
		width: 100%;
		padding: 0.5rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		font-family: inherit;
		font-size: inherit;
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-editor__sections {
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		padding: var(--mp-spacing-md, 1rem);
		margin-bottom: var(--mp-spacing-md, 1rem);
	}
	.mp-editor__section {
		margin-bottom: var(--mp-spacing-md, 1rem);
		padding-bottom: var(--mp-spacing-md, 1rem);
		border-bottom: 1px solid var(--mp-color-border, #e5e5e5);
	}
	.mp-editor__section input,
	.mp-editor__section textarea {
		width: 100%;
		padding: 0.5rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		font-family: inherit;
		margin-bottom: 0.5rem;
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-editor__section textarea {
		font-family: var(--mp-font-mono, 'JetBrains Mono', monospace);
		font-size: 0.9em;
	}
	.mp-editor__remove {
		font-size: 0.8em;
		color: var(--mp-color-error, #dc2626);
		background: none;
		border: none;
		cursor: pointer;
	}
	.mp-editor__add-section {
		background: none;
		border: 1px dashed var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		padding: 0.5rem 1rem;
		cursor: pointer;
		color: var(--mp-color-link, #2563eb);
		width: 100%;
	}
	.mp-editor__actions {
		display: flex;
		gap: 0.5rem;
	}
	.mp-editor__save {
		padding: 0.5rem 1.5rem;
		background: var(--mp-color-primary, #2563eb);
		color: white;
		border: none;
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
		font-weight: 600;
	}
	.mp-editor__save:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.mp-editor__cancel {
		padding: 0.5rem 1.5rem;
		background: none;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
	}
	.mp-editor__error {
		background: #fef2f2;
		color: var(--mp-color-error, #dc2626);
		padding: 0.5rem 1rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: var(--mp-spacing-md, 1rem);
	}
</style>
