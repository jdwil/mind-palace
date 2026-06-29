<script lang="ts">
	import { MindPalaceProvider, WikiSearch, WikiBrowser, WikiGraph } from '$lib/index.js';

	let selectedSlug = $state<string | null>(null);
</script>

<h1>Mind Palace UI — Dev</h1>
<p>Configure <code>apiUrl</code> to point at your running mind-palace-web server.</p>

<MindPalaceProvider apiUrl="http://localhost:3000" token="">
	<WikiSearch onSelect={(slug) => selectedSlug = slug} />
	<hr />
	<WikiBrowser onSelect={(slug) => selectedSlug = slug} />
	<hr />
	<WikiGraph onSelectNode={(slug) => selectedSlug = slug} />
	{#if selectedSlug}
		<p>Selected: <code>{selectedSlug}</code></p>
	{/if}
</MindPalaceProvider>
