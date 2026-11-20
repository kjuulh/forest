(function() {
    const PATTERN_TYPES = [
        { value: 'branch_pattern', label: 'Branch', placeholder: 'e.g. main' },
        { value: 'title_pattern', label: 'Title', placeholder: 'e.g. ^Release.*' },
        { value: 'author_pattern', label: 'Author', placeholder: 'e.g. ci-bot' },
        { value: 'commit_message_pattern', label: 'Commit message', placeholder: 'e.g. ^Merge pull request.*' },
        { value: 'source_type_pattern', label: 'Source type', placeholder: 'e.g. github-actions' },
    ];

    let controller = null;

    function init() {
        const form = document.getElementById('trigger-form');
        if (form && form.dataset.triggerFormReady) return;
        if (controller) controller.abort();
        controller = null;
        if (!form) return;
        form.dataset.triggerFormReady = '1';
        controller = new AbortController();
        const signal = controller.signal;

        const rows = document.getElementById('pattern-rows');
        const addBtn = document.getElementById('add-pattern-btn');
        const nameHidden = document.getElementById('trigger-name-hidden');
        const namePreview = document.getElementById('trigger-name-preview');

        function updateName() {
            const parts = [];
            rows.querySelectorAll('.pattern-row').forEach(row => {
                const val = row.querySelector('input[type="text"]')?.value?.trim();
                if (val) parts.push(val);
            });
            const envs = [];
            document.querySelectorAll('input[name="target_environments"]:checked').forEach(cb => {
                envs.push(cb.value);
            });
            let name = '';
            if (parts.length > 0 && envs.length > 0) {
                name = parts[0] + '-to-' + envs.join('-');
            } else if (parts.length > 0) {
                name = parts[0];
            }
            nameHidden.value = name;
            namePreview.textContent = name || '(auto-generated from pattern)';
        }

        function addRow() {
            const used = new Set();
            rows.querySelectorAll('select').forEach(sel => used.add(sel.value));
            const available = PATTERN_TYPES.filter(t => !used.has(t.value));
            if (available.length === 0) return;

            const type = available[0];
            const row = document.createElement('div');
            row.className = 'pattern-row flex items-center gap-2';
            row.innerHTML = `
                <select name="_pattern_type" class="border border-gray-300 rounded-md px-2 py-1.5 text-sm bg-white min-w-[140px]">
                    ${PATTERN_TYPES.map(t =>
                        `<option value="${t.value}" ${t.value === type.value ? 'selected' : ''} ${used.has(t.value) && t.value !== type.value ? 'disabled' : ''}>${t.label}</option>`
                    ).join('')}
                </select>
                <input type="text" name="${type.value}" placeholder="${type.placeholder}"
                    class="flex-1 border border-gray-300 rounded-md px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-gray-900 font-mono">
                <button type="button" class="remove-btn text-gray-400 hover:text-red-500 p-1" title="Remove">
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>
                </button>
            `;

            const select = row.querySelector('select');
            const input = row.querySelector('input[type="text"]');

            select.addEventListener('change', () => {
                const newType = PATTERN_TYPES.find(t => t.value === select.value);
                input.name = select.value;
                input.placeholder = newType?.placeholder || '';
                updateName();
                updateAvailable();
            }, { signal: signal });
            input.addEventListener('input', updateName, { signal: signal });
            row.querySelector('.remove-btn').addEventListener('click', () => {
                row.remove();
                updateName();
                updateAvailable();
            }, { signal: signal });

            rows.appendChild(row);
            input.focus();
            updateName();
            updateAvailable();
        }

        function updateAvailable() {
            const used = new Set();
            rows.querySelectorAll('select').forEach(sel => used.add(sel.value));
            rows.querySelectorAll('select').forEach(sel => {
                const current = sel.value;
                sel.querySelectorAll('option').forEach(opt => {
                    opt.disabled = used.has(opt.value) && opt.value !== current;
                });
            });
            addBtn.style.display = used.size >= PATTERN_TYPES.length ? 'none' : '';
        }

        var pipeline = document.getElementById('use-pipeline');
        var pipelineRow = document.getElementById('pipeline-select-row');
        if (pipeline && pipelineRow) {
            pipeline.addEventListener('change', function () {
                pipelineRow.classList.toggle('hidden', !pipeline.checked);
            }, { signal: signal });
        }

        addBtn.addEventListener('click', addRow, { signal: signal });
        document.querySelectorAll('input[name="target_environments"]').forEach(cb => {
            cb.addEventListener('change', updateName, { signal: signal });
        });

        addRow();
    }

    init();
    document.addEventListener('sedx:load', init);
})();
