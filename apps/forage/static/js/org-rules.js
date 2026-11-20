(function() {
    let projects = [];
    let ruleSets = [];
    let controller = null;
    let signal = null;

    function readJson(id) {
        const el = document.getElementById(id);
        if (!el) return [];
        try {
            return JSON.parse(el.textContent || '[]') || [];
        } catch (e) {
            return [];
        }
    }

    function splitCsvLines(value) {
        return (value || '')
            .split(/[,\n\r]/)
            .map(v => v.trim())
            .filter(Boolean);
    }

    function parseMetadataMatch(value) {
        const pairs = {};
        for (const raw of (value || '').split(/\n/)) {
            const line = raw.trim();
            if (!line) continue;
            const idx = line.indexOf('=');
            if (idx === -1) {
                return { error: 'metadata selector `' + line + '` must use key=value' };
            }
            const key = line.slice(0, idx).trim();
            if (!key) {
                return { error: 'metadata selector `' + line + '` has an empty key' };
            }
            pairs[key] = line.slice(idx + 1).trim();
        }
        return { pairs };
    }

    function metadataValue(metadata, key) {
        let current = metadata || {};
        for (const part of key.split('.')) {
            if (!part || current == null || typeof current !== 'object' || !(part in current)) return undefined;
            current = current[part];
        }
        return typeof current === 'string' ? current : undefined;
    }

    function matchesSelector(selector) {
        const include = selector.include_projects || [];
        const exclude = selector.exclude_projects || [];
        const tags = selector.tags || [];
        const metadataMatch = selector.metadata_match || {};
        let nameRegex = null;
        if (selector.name_regex) {
            try {
                nameRegex = new RegExp(selector.name_regex);
            } catch (err) {
                return { error: 'Invalid name regex: ' + err.message, matches: [] };
            }
        }

        const matches = projects.filter(project => {
            if (include.length > 0 && !include.includes(project.name)) return false;
            if (exclude.includes(project.name)) return false;
            if (nameRegex && !nameRegex.test(project.name)) return false;

            const metadata = project.metadata || {};
            for (const [key, expected] of Object.entries(metadataMatch)) {
                if (metadataValue(metadata, key) !== expected) return false;
            }

            if (tags.length > 0) {
                const projectTags = Array.isArray(metadata.tags)
                    ? metadata.tags
                    : String(metadata.tags || '').split(',').map(v => v.trim()).filter(Boolean);
                if (!tags.every(tag => projectTags.includes(tag))) return false;
            }

            return true;
        });
        return { matches };
    }

    function renderPreview(target, result) {
        target.innerHTML = '';
        if (result.error) {
            const err = document.createElement('p');
            err.className = 'text-sm text-red-600';
            err.textContent = result.error;
            target.appendChild(err);
            return;
        }

        const count = document.createElement('p');
        count.className = 'text-xs font-semibold uppercase tracking-wide text-gray-500 mb-2';
        count.textContent = result.matches.length + ' matching project' + (result.matches.length === 1 ? '' : 's');
        target.appendChild(count);

        if (projects.length === 0) {
            const empty = document.createElement('p');
            empty.className = 'text-gray-500';
            empty.textContent = 'No projects found in this organisation.';
            target.appendChild(empty);
            return;
        }
        if (result.matches.length === 0) {
            const empty = document.createElement('p');
            empty.className = 'text-gray-500';
            empty.textContent = 'No matching projects.';
            target.appendChild(empty);
            return;
        }

        const chips = document.createElement('div');
        chips.className = 'flex flex-wrap gap-1.5';
        for (const project of result.matches.slice(0, 12)) {
            const chip = document.createElement('span');
            chip.className = 'inline-flex rounded bg-white border border-gray-200 px-2 py-0.5 text-xs font-medium text-gray-700';
            chip.textContent = project.name;
            chips.appendChild(chip);
        }
        if (result.matches.length > 12) {
            const more = document.createElement('span');
            more.className = 'inline-flex rounded bg-white border border-gray-200 px-2 py-0.5 text-xs text-gray-500';
            more.textContent = '+' + (result.matches.length - 12) + ' more';
            chips.appendChild(more);
        }
        target.appendChild(chips);
    }

    function setupPolicyBuilder() {
        const type = document.getElementById('org-policy-type');
        if (!type) return;
        const sections = {
            soak_time: document.getElementById('org-policy-soak-fields'),
            branch_restriction: document.getElementById('org-policy-branch-fields'),
            approval: document.getElementById('org-policy-approval-fields'),
            supersede_pending: document.getElementById('org-policy-supersede-fields'),
        };
        function toggle() {
            for (const [key, el] of Object.entries(sections)) {
                if (!el) continue;
                const active = key === type.value;
                el.classList.toggle('hidden', !active);
                el.querySelectorAll('input').forEach(input => input.disabled = !active);
            }
        }
        type.addEventListener('change', toggle, { signal });
        toggle();
    }

    function setupTriggerBuilder() {
        const rows = document.getElementById('org-trigger-pattern-rows');
        const addBtn = document.getElementById('org-add-pattern-btn');
        if (!rows || !addBtn) return;
        const patternTypes = [
            { value: 'branch_pattern', label: 'Branch', placeholder: 'e.g. ^main$' },
            { value: 'title_pattern', label: 'Title', placeholder: 'e.g. ^Release.*' },
            { value: 'author_pattern', label: 'Author', placeholder: 'e.g. ci-bot' },
            { value: 'commit_message_pattern', label: 'Commit message', placeholder: 'e.g. ^Merge pull request.*' },
            { value: 'source_type_pattern', label: 'Source type', placeholder: 'e.g. woodpecker' },
        ];
        function usedTypes() {
            return new Set(Array.from(rows.querySelectorAll('select')).map(sel => sel.value));
        }
        function refreshOptions() {
            const used = usedTypes();
            rows.querySelectorAll('select').forEach(sel => {
                const current = sel.value;
                sel.querySelectorAll('option').forEach(opt => {
                    opt.disabled = used.has(opt.value) && opt.value !== current;
                });
            });
            addBtn.classList.toggle('hidden', used.size >= patternTypes.length);
        }
        function addPatternRow() {
            const used = usedTypes();
            const type = patternTypes.find(item => !used.has(item.value));
            if (!type) return;
            const row = document.createElement('div');
            row.className = 'flex items-center gap-2';
            row.innerHTML = `
                <select class="w-40 rounded-md border border-gray-300 px-2 py-1.5 text-sm bg-white">
                    ${patternTypes.map(item => `<option value="${item.value}" ${item.value === type.value ? 'selected' : ''}>${item.label}</option>`).join('')}
                </select>
                <input class="flex-1 rounded-md border border-gray-300 px-3 py-1.5 text-sm font-mono" placeholder="${type.placeholder}">
                <button type="button" class="text-gray-400 hover:text-red-500" title="Remove">×</button>
            `;
            const select = row.querySelector('select');
            const input = row.querySelector('input');
            select.addEventListener('change', () => {
                const selected = patternTypes.find(item => item.value === select.value);
                input.placeholder = selected ? selected.placeholder : '';
                refreshOptions();
            }, { signal });
            row.querySelector('button').addEventListener('click', () => {
                row.remove();
                refreshOptions();
            }, { signal });
            rows.appendChild(row);
            refreshOptions();
        }
        addBtn.addEventListener('click', addPatternRow, { signal });
        addPatternRow();
    }

    function slugify(value, fallback) {
        const slug = (value || '')
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, '-')
            .replace(/^-+|-+$/g, '');
        return slug || fallback;
    }

    function buildPolicyFromForm() {
        const name = document.getElementById('org-policy-name')?.value.trim();
        if (!name) return null;
        const type = document.getElementById('org-policy-type').value;
        if (type === 'soak_time') {
            const source = document.getElementById('org-policy-source-env').value.trim();
            const target = document.getElementById('org-policy-soak-target-env').value.trim();
            const duration = Number(document.getElementById('org-policy-duration').value);
            if (!source || !target || !Number.isFinite(duration) || duration < 1) {
                throw new Error('Soak policies require source environment, target environment, and a positive duration.');
            }
            return { name, enabled: true, config: { SoakTime: { source_environment: source, target_environment: target, duration_seconds: duration } } };
        }
        if (type === 'approval') {
            const target = document.getElementById('org-policy-approval-target-env').value.trim();
            const approvals = Number(document.getElementById('org-policy-approvals').value || '1');
            if (!target || !Number.isFinite(approvals) || approvals < 1) {
                throw new Error('Approval policies require a target environment and at least one approval.');
            }
            return { name, enabled: true, config: { Approval: { target_environment: target, required_approvals: approvals } } };
        }
        if (type === 'supersede_pending') {
            const target = document.getElementById('org-policy-supersede-target-env').value.trim();
            if (!target) {
                throw new Error('Supersede pending policies require a target environment.');
            }
            const sameBranchOnly = document.getElementById('org-policy-supersede-same-branch').checked;
            return { name, enabled: true, config: { SupersedePending: { target_environment: target, same_branch_only: sameBranchOnly } } };
        }
        const target = document.getElementById('org-policy-branch-target-env').value.trim();
        const branch = document.getElementById('org-policy-branch-pattern').value.trim();
        if (!target || !branch) {
            throw new Error('Branch policies require a target environment and branch pattern.');
        }
        return { name, enabled: true, config: { BranchRestriction: { target_environment: target, branch_pattern: branch } } };
    }

    function buildTriggerFromForm() {
        const patterns = {};
        document.querySelectorAll('#org-trigger-pattern-rows > div').forEach(row => {
            const key = row.querySelector('select').value;
            const value = row.querySelector('input').value.trim();
            if (value) patterns[key] = value;
        });
        const targetEnvironments = splitCsvLines(document.getElementById('org-trigger-target-envs')?.value || '');
        const targetDestinations = splitCsvLines(document.getElementById('org-trigger-target-dests')?.value || '');
        const forceRelease = document.getElementById('org-trigger-force')?.checked || false;
        const usePipeline = document.getElementById('org-trigger-pipeline')?.checked || false;
        const explicitName = document.getElementById('org-trigger-name')?.value.trim();
        const hasTrigger = explicitName || Object.keys(patterns).length > 0 || targetEnvironments.length > 0 || targetDestinations.length > 0 || forceRelease || usePipeline;
        if (!hasTrigger) return null;
        if (!usePipeline && targetEnvironments.length === 0 && targetDestinations.length === 0) {
            throw new Error('Triggers must target at least one environment or destination unless they use a release pipeline.');
        }
        const firstPattern = Object.values(patterns)[0] || 'release';
        const targetPart = targetEnvironments[0] || targetDestinations[0] || 'pipeline';
        return {
            name: explicitName || slugify(firstPattern + '-to-' + targetPart, 'org-trigger'),
            enabled: true,
            branch_pattern: patterns.branch_pattern || null,
            title_pattern: patterns.title_pattern || null,
            author_pattern: patterns.author_pattern || null,
            commit_message_pattern: patterns.commit_message_pattern || null,
            source_type_pattern: patterns.source_type_pattern || null,
            target_environments: targetEnvironments,
            target_destinations: targetDestinations,
            force_release: forceRelease,
            use_pipeline: usePipeline,
        };
    }

    function buildPipelineFromForm() {
        const name = document.getElementById('org-pipeline-name')?.value.trim();
        const rawStages = document.getElementById('org-pipeline-stages')?.value.trim();
        if (!name && !rawStages) return null;
        if (!name) throw new Error('Pipelines require a name.');
        const stages = rawStages ? JSON.parse(rawStages) : [];
        if (!Array.isArray(stages) || stages.length === 0) {
            throw new Error('Pipelines require at least one stage.');
        }
        return { name, enabled: true, stages };
    }

    function wireRuleBuilders(form) {
        setupPolicyBuilder();
        setupTriggerBuilder();
        form.addEventListener('submit', event => {
            try {
                const policy = buildPolicyFromForm();
                if (policy) form.elements.policies_json.value = JSON.stringify([policy], null, 2);
                const trigger = buildTriggerFromForm();
                if (trigger) form.elements.triggers_json.value = JSON.stringify([trigger], null, 2);
                const pipeline = buildPipelineFromForm();
                if (pipeline) form.elements.pipelines_json.value = JSON.stringify([pipeline], null, 2);
            } catch (err) {
                event.preventDefault();
                alert(err.message);
            }
        }, { signal });
    }

    function init() {
        const root = document.getElementById('org-rule-projects-data');
        if (root && root.dataset.orgRulesReady) return;
        if (controller) controller.abort();
        controller = null;
        signal = null;
        if (!root) return;
        root.dataset.orgRulesReady = '1';
        controller = new AbortController();
        signal = controller.signal;
        projects = readJson('org-rule-projects-data');
        ruleSets = readJson('org-rule-sets-data');

        document.querySelectorAll('[data-rule-preview]').forEach(target => {
            const name = target.getAttribute('data-rule-preview');
            const rule = ruleSets.find(item => item.name === name);
            renderPreview(target, matchesSelector((rule && rule.selector) || {}));
        });

        const form = document.getElementById('org-rule-form');
        const livePreview = document.getElementById('new-rule-preview');
        if (form && livePreview) {
            function updateLivePreview() {
                const metadata = parseMetadataMatch(form.elements.metadata_match.value);
                if (metadata.error) {
                    renderPreview(livePreview, { error: metadata.error, matches: [] });
                    return;
                }
                renderPreview(livePreview, matchesSelector({
                    include_projects: splitCsvLines(form.elements.include_projects.value),
                    exclude_projects: splitCsvLines(form.elements.exclude_projects.value),
                    name_regex: form.elements.name_regex.value.trim() || null,
                    metadata_match: metadata.pairs,
                    tags: splitCsvLines(form.elements.tags.value),
                }));
            }
            ['include_projects', 'exclude_projects', 'name_regex', 'metadata_match', 'tags'].forEach(name => {
                form.elements[name].addEventListener('input', updateLivePreview, { signal });
            });
            wireRuleBuilders(form);
            updateLivePreview();
        }
    }

    init();
    document.addEventListener('sedx:load', init);
})();
