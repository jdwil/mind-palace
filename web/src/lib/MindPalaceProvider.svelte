<script lang="ts">
	import type { Snippet } from 'svelte';
	import { createClient, setClient } from './client.js';

	interface Props {
		apiUrl: string;
		token?: string;
		children: Snippet;
	}

	let { apiUrl, token, children }: Props = $props();

	const client = createClient(apiUrl, token);
	setClient(client);
</script>

<div class="mp-provider">
	{@render children()}
</div>

<style>
	.mp-provider {
		font-family: var(--mp-font-family, system-ui, -apple-system, sans-serif);
		color: var(--mp-color-text, #1a1a1a);
		background: var(--mp-color-bg, #ffffff);
		line-height: var(--mp-line-height, 1.6);
	}
</style>
