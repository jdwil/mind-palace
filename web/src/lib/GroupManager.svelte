<script lang="ts">
	import { getClient, type GroupView } from './client.js';

	const client = getClient();
	let groups: GroupView[] = $state([]);
	let selected: GroupView | null = $state(null);
	let loading = $state(true);
	let error: string | null = $state(null);
	let busy = $state(false);

	// Create-group form
	let newId = $state('');
	let newName = $state('');

	// Add member/manager forms (per selected group)
	let memberEmail = $state('');
	let managerEmail = $state('');

	async function loadGroups() {
		loading = true;
		error = null;
		try {
			groups = await client.listGroups();
			// Refresh the selected group reference if it still exists.
			if (selected) {
				selected = groups.find((g) => g.id === selected!.id) ?? null;
			}
		} catch (e: any) {
			error = e.message;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		loadGroups();
	});

	async function createGroup(e: Event) {
		e.preventDefault();
		if (!newId.trim() || !newName.trim()) return;
		busy = true;
		error = null;
		try {
			await client.createGroup({ id: newId.trim(), name: newName.trim() });
			newId = '';
			newName = '';
			await loadGroups();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	function select(g: GroupView) {
		selected = g;
		memberEmail = '';
		managerEmail = '';
	}

	async function run(fn: () => Promise<GroupView>) {
		busy = true;
		error = null;
		try {
			const updated = await fn();
			selected = updated;
			// Keep the list in sync.
			groups = groups.map((g) => (g.id === updated.id ? updated : g));
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	async function addMember(e: Event) {
		e.preventDefault();
		if (!selected || !memberEmail.trim()) return;
		const email = memberEmail.trim();
		await run(() => client.addGroupMember(selected!.id, email));
		memberEmail = '';
	}

	async function addManager(e: Event) {
		e.preventDefault();
		if (!selected || !managerEmail.trim()) return;
		const email = managerEmail.trim();
		await run(() => client.addGroupManager(selected!.id, email));
		managerEmail = '';
	}
</script>

<section class="mp-groups" aria-label="Group management">
	<h3 class="mp-groups__heading">Groups</h3>

	{#if error}
		<div class="mp-groups__error">{error}</div>
	{/if}

	<div class="mp-groups__layout">
		<div class="mp-groups__list-pane">
			{#if loading}
				<div class="mp-groups__loading">Loading…</div>
			{:else}
				<ul class="mp-groups__list">
					{#each groups as g}
						<li>
							<button
								class="mp-groups__item"
								class:mp-groups__item--active={selected?.id === g.id}
								onclick={() => select(g)}
							>
								<span class="mp-groups__item-name">{g.name}</span>
								<span class="mp-groups__item-id">{g.id}</span>
							</button>
						</li>
					{/each}
				</ul>
			{/if}

			<form class="mp-groups__create" onsubmit={createGroup}>
				<input bind:value={newId} placeholder="group-id" aria-label="Group id" />
				<input bind:value={newName} placeholder="Display name" aria-label="Group name" />
				<button type="submit" disabled={busy || !newId.trim() || !newName.trim()}>Create</button>
			</form>
		</div>

		<div class="mp-groups__detail-pane">
			{#if selected}
				<h4 class="mp-groups__detail-title">{selected.name}</h4>

				<div class="mp-groups__section">
					<h5>Managers</h5>
					<ul class="mp-groups__chips">
						{#each selected.managers as m}
							<li class="mp-groups__chip">
								{m}
								{#if selected.can_manage}
									<button
										aria-label={`Remove manager ${m}`}
										onclick={() => run(() => client.removeGroupManager(selected!.id, m))}
										disabled={busy}
									>×</button>
								{/if}
							</li>
						{/each}
					</ul>
					{#if selected.can_manage}
						<form class="mp-groups__add" onsubmit={addManager}>
							<input bind:value={managerEmail} placeholder="manager@example.com" aria-label="Manager email" />
							<button type="submit" disabled={busy || !managerEmail.trim()}>Add manager</button>
						</form>
					{/if}
				</div>

				<div class="mp-groups__section">
					<h5>Members</h5>
					<ul class="mp-groups__chips">
						{#each selected.members as m}
							<li class="mp-groups__chip">
								{m}
								{#if selected.can_manage}
									<button
										aria-label={`Remove member ${m}`}
										onclick={() => run(() => client.removeGroupMember(selected!.id, m))}
										disabled={busy}
									>×</button>
								{/if}
							</li>
						{/each}
					</ul>
					{#if selected.can_manage}
						<form class="mp-groups__add" onsubmit={addMember}>
							<input bind:value={memberEmail} placeholder="member@example.com" aria-label="Member email" />
							<button type="submit" disabled={busy || !memberEmail.trim()}>Add member</button>
						</form>
					{/if}
				</div>

				{#if selected.can_manage}
					<p class="mp-groups__note">
						Membership and manager changes are manager-only; the server enforces this even if a
						control is shown.
					</p>
				{:else}
					<p class="mp-groups__note">
						You are not a manager of this group, so membership controls are hidden. (The server
						enforces this regardless.)
					</p>
				{/if}
			{:else}
				<p class="mp-groups__empty">Select a group to manage its members and managers.</p>
			{/if}
		</div>
	</div>
</section>

<style>
	.mp-groups__heading {
		margin: 0 0 var(--mp-spacing-md, 1rem);
		font-size: 1rem;
		color: var(--mp-color-heading, #111);
	}
	.mp-groups__layout {
		display: flex;
		gap: var(--mp-spacing-md, 1rem);
		flex-wrap: wrap;
	}
	.mp-groups__list-pane {
		flex: 1;
		min-width: 12rem;
	}
	.mp-groups__detail-pane {
		flex: 2;
		min-width: 16rem;
	}
	.mp-groups__list {
		list-style: none;
		margin: 0 0 0.75rem;
		padding: 0;
	}
	.mp-groups__item {
		display: flex;
		flex-direction: column;
		width: 100%;
		text-align: left;
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-bg, #fff);
		cursor: pointer;
		margin-bottom: 0.35rem;
	}
	.mp-groups__item--active {
		border-color: var(--mp-color-primary, #2563eb);
		background: var(--mp-color-surface, #f9fafb);
	}
	.mp-groups__item-name {
		font-weight: 600;
	}
	.mp-groups__item-id {
		font-size: 0.75em;
		color: var(--mp-color-muted, #6b7280);
		font-family: var(--mp-font-mono, monospace);
	}
	.mp-groups__create,
	.mp-groups__add {
		display: flex;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.mp-groups__add {
		margin-top: 0.5rem;
	}
	.mp-groups__create input,
	.mp-groups__add input {
		flex: 1;
		min-width: 7rem;
		padding: 0.4rem;
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		background: var(--mp-color-bg, #fff);
		color: var(--mp-color-text, #1a1a1a);
	}
	.mp-groups__create button,
	.mp-groups__add button {
		padding: 0.4rem 0.9rem;
		background: var(--mp-color-primary, #2563eb);
		color: white;
		border: none;
		border-radius: var(--mp-radius, 4px);
		cursor: pointer;
		font-weight: 600;
		font-size: 0.85em;
	}
	.mp-groups__create button:disabled,
	.mp-groups__add button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.mp-groups__section {
		margin-bottom: var(--mp-spacing-md, 1rem);
	}
	.mp-groups__section h5 {
		margin: 0 0 0.5rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-groups__chips {
		list-style: none;
		margin: 0 0 0.5rem;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
	}
	.mp-groups__chip {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		padding: 0.2rem 0.5rem;
		background: var(--mp-color-badge-bg, #f3f4f6);
		border-radius: var(--mp-radius, 4px);
		font-size: 0.8em;
		font-family: var(--mp-font-mono, monospace);
	}
	.mp-groups__chip button {
		background: none;
		border: none;
		color: var(--mp-color-error, #dc2626);
		cursor: pointer;
		font-size: 1em;
		line-height: 1;
	}
	.mp-groups__detail-title {
		margin: 0 0 var(--mp-spacing-md, 1rem);
	}
	.mp-groups__empty,
	.mp-groups__note {
		font-size: 0.85em;
		color: var(--mp-color-muted, #6b7280);
	}
	.mp-groups__loading {
		color: var(--mp-color-muted, #6b7280);
		font-size: 0.9em;
	}
	.mp-groups__error {
		background: #fef2f2;
		color: var(--mp-color-error, #dc2626);
		padding: 0.5rem 0.75rem;
		border-radius: var(--mp-radius, 4px);
		margin-bottom: 0.75rem;
		font-size: 0.85em;
	}
</style>
