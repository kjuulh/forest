(function() {
    const CREATE_ROW_INPUT = 'flex-1 text-sm px-3 py-2 border border-gray-200 rounded-md focus:outline-none focus:ring-1 focus:ring-green-500 font-mono';
    const DETAIL_ROW_INPUT = 'flex-1 text-sm px-3 py-2 border border-gray-200 rounded-md focus:outline-none focus:ring-2 focus:ring-green-500 focus:border-transparent font-mono';
    const REMOVE_BUTTON = '<button type="button" class="remove-meta-row text-gray-300 hover:text-red-500 transition-colors p-1" title="Remove row">'
        + '<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path></svg>'
        + '</button>';

    function rowHtml(inputClass, secretTitle) {
        return '<input type="text" name="metadata_keys" placeholder="key" class="' + inputClass + '">'
            + '<input type="text" name="metadata_values" placeholder="value" class="' + inputClass + '">'
            + '<label class="shrink-0 flex items-center gap-1 text-xs text-gray-400 hover:text-gray-600 cursor-pointer" title="' + secretTitle + '">'
            + '<input type="checkbox" class="meta-sensitive accent-green-600">secret'
            + '</label>'
            + REMOVE_BUTTON;
    }

    function addMetaRow(container) {
        const row = document.createElement('div');
        row.className = 'flex gap-2 items-center metadata-row';
        row.innerHTML = rowHtml(CREATE_ROW_INPUT, 'Mark as a credential: the value is hidden after saving');
        container.appendChild(row);
        row.querySelector('input').focus();
        return row;
    }

    function readDestinationTypes() {
        const el = document.getElementById('destination-types');
        if (!el) return [];
        try {
            return JSON.parse(el.textContent) || [];
        } catch (e) {
            return [];
        }
    }

    let destTypes = [];

    function setSection(container, visible) {
        container.classList.toggle('hidden', !visible);
        container.querySelectorAll('input').forEach(function(el) { el.disabled = !visible; });
    }

    function updateTypeFields(idx) {
        const select = document.getElementById('destination_type_' + idx);
        const typeFieldsContainer = document.getElementById('type-fields-' + idx);
        const customContainer = document.getElementById('custom-fields-' + idx);
        const extraContainer = document.getElementById('extra-fields-' + idx);
        const orgInput = document.getElementById('type_organisation_' + idx);
        const nameInput = document.getElementById('type_name_' + idx);
        const versionInput = document.getElementById('type_version_' + idx);
        if (!select || !typeFieldsContainer || !customContainer || !extraContainer) return;

        const val = select.value;

        orgInput.value = '';
        nameInput.value = '';
        versionInput.value = '';

        if (val === '' || val === 'custom') {
            typeFieldsContainer.classList.add('hidden');
            typeFieldsContainer.innerHTML = '';
            setSection(extraContainer, false);
            setSection(customContainer, true);
            return;
        }

        setSection(customContainer, false);
        setSection(extraContainer, true);

        const type = destTypes.find(function(t) {
            return (t.organisation + '/' + t.name + '@' + t.version) === val;
        });
        if (!type) return;

        orgInput.value = type.organisation;
        nameInput.value = type.name;
        versionInput.value = type.version;

        typeFieldsContainer.classList.remove('hidden');
        typeFieldsContainer.innerHTML = '';

        if (!type.fields || type.fields.length === 0) {
            typeFieldsContainer.innerHTML = '<p class="text-xs text-gray-400">This type has no required fields.</p>';
            return;
        }

        type.fields.forEach(function(field) {
            const inputType = (field.sensitive || field.field_type === 'secret') ? 'password'
                            : field.field_type === 'number' ? 'number'
                            : field.field_type === 'url'    ? 'url'
                            : 'text';

            const wrapper = document.createElement('div');
            wrapper.className = 'space-y-0.5';

            const label = document.createElement('label');
            label.className = 'block text-xs font-medium text-gray-600';
            label.textContent = field.label || field.name;
            if (field.required) {
                const req = document.createElement('span');
                req.className = 'text-red-500 ml-0.5';
                req.textContent = '*';
                label.appendChild(req);
            }
            if (field.description) {
                const hint = document.createElement('span');
                hint.className = 'ml-1 text-gray-400 font-normal';
                hint.textContent = '— ' + field.description;
                label.appendChild(hint);
            }

            const keyInput = document.createElement('input');
            keyInput.type = 'hidden';
            keyInput.name = 'metadata_keys';
            keyInput.value = field.name;

            const valueInput = document.createElement('input');
            valueInput.type = inputType;
            valueInput.name = 'metadata_values';
            valueInput.placeholder = field.default_value || '';
            valueInput.required = field.required;
            valueInput.className = 'w-full text-sm px-3 py-2 border border-gray-200 rounded-md focus:outline-none focus:ring-1 focus:ring-green-500 font-mono';

            wrapper.appendChild(label);
            wrapper.appendChild(keyInput);
            wrapper.appendChild(valueInput);
            typeFieldsContainer.appendChild(wrapper);
        });
    }

    function fillRows(container, metadata, skip) {
        Object.entries(metadata).forEach(function(entry) {
            if (skip && skip.has(entry[0])) return;
            const inputs = addMetaRow(container).querySelectorAll('input');
            inputs[0].value = entry[0];
            inputs[1].value = entry[1];
        });
    }

    function importDestination(idx) {
        const textarea = document.getElementById('import-json-' + idx);
        if (!textarea) return;
        let data;
        try {
            data = JSON.parse(textarea.value);
        } catch (e) {
            alert('Invalid JSON: ' + e.message);
            return;
        }

        const form = document.getElementById('create-dest-form-' + idx);
        if (!form) return;

        const nameInput = form.querySelector('input[name="name"]');
        if (nameInput && data.name) nameInput.value = data.name;

        if (data.type && data.type.name) {
            const typeKey = data.type.organisation + '/' + data.type.name + '@' + data.type.version;
            const select = document.getElementById('destination_type_' + idx);
            if (select) {
                for (const opt of select.options) {
                    if (opt.value.startsWith(typeKey)) {
                        select.value = opt.value;
                        break;
                    }
                }
                updateTypeFields(idx);
            }
        }

        if (data.metadata) {
            const typeFieldsContainer = document.getElementById('type-fields-' + idx);
            const extraContainer = document.getElementById('extra-fields-' + idx);

            if (typeFieldsContainer && !typeFieldsContainer.classList.contains('hidden')) {
                const schemaKeys = new Set();
                typeFieldsContainer.querySelectorAll('input[name="metadata_keys"]').forEach(function(keyInput) {
                    schemaKeys.add(keyInput.value);
                    const valueInput = keyInput.nextElementSibling;
                    if (valueInput && data.metadata[keyInput.value] !== undefined) {
                        valueInput.value = data.metadata[keyInput.value];
                    }
                });

                if (extraContainer) {
                    const extraRows = extraContainer.querySelector('.extra-meta-rows');
                    extraRows.innerHTML = '';
                    fillRows(extraRows, data.metadata, schemaKeys);
                }
            } else {
                const customContainer = document.getElementById('custom-fields-' + idx);
                if (customContainer) {
                    const rowsContainer = customContainer.querySelector('.create-meta-rows');
                    rowsContainer.innerHTML = '';
                    fillRows(rowsContainer, data.metadata, null);
                }
            }
        }

        textarea.value = '';
        textarea.closest('details').removeAttribute('open');
    }

    function pasteAndImport(idx) {
        const textarea = document.getElementById('import-json-' + idx);
        if (!textarea) return;
        if (navigator.clipboard && navigator.clipboard.readText) {
            navigator.clipboard.readText().then(function(text) {
                textarea.value = text;
                importDestination(idx);
            }).catch(function() {
                textarea.focus();
            });
        } else {
            textarea.focus();
        }
    }

    function exportDestination(btn) {
        const dest = {
            name: btn.dataset.name,
            environment: btn.dataset.env,
            type: {
                organisation: btn.dataset.typeOrg || '',
                name: btn.dataset.typeName || '',
                version: parseInt(btn.dataset.typeVersion) || 0,
            },
            metadata: JSON.parse(btn.dataset.metadata || '{}'),
            sensitive_keys_omitted: JSON.parse(btn.dataset.sensitiveKeys || '[]'),
        };
        const json = JSON.stringify(dest, null, 2);
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(json).then(function() {
                const badge = document.createElement('span');
                badge.textContent = 'Copied!';
                badge.className = 'text-xs text-green-600 font-medium ml-1';
                btn.parentElement.insertBefore(badge, btn.nextSibling);
                setTimeout(function() { badge.remove(); }, 1500);
            });
        } else {
            const blob = new Blob([json], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = dest.name + '.json';
            a.click();
            URL.revokeObjectURL(url);
        }
    }

    function revealValue(btn) {
        const form = btn.closest('form');
        const input = btn.parentElement.querySelector('input[name="metadata_values"]');
        if (!form || !input) return;

        if (btn.dataset.revealed === '1') {
            input.value = '';
            input.placeholder = 'hidden — leave blank to keep, or type to replace';
            btn.textContent = 'Reveal';
            btn.dataset.revealed = '0';
            return;
        }

        btn.disabled = true;
        btn.textContent = '…';
        const url = form.dataset.revealUrl + '&key=' + encodeURIComponent(btn.dataset.key);

        fetch(url, { headers: { 'Accept': 'text/plain' } })
            .then(function(res) {
                if (!res.ok) throw new Error('reveal failed (' + res.status + ')');
                return res.text();
            })
            .then(function(value) {
                input.value = value;
                btn.textContent = 'Hide';
                btn.dataset.revealed = '1';
            })
            .catch(function(err) {
                btn.textContent = 'Reveal';
                alert(err.message);
            })
            .finally(function() { btn.disabled = false; });
    }

    document.addEventListener('click', function(e) {
        const target = e.target instanceof Element ? e.target : null;
        if (!target) return;
        const remove = target.closest('.remove-meta-row');
        if (remove) {
            const row = remove.closest('.metadata-row');
            if (row) row.remove();
            return;
        }
        const add = target.closest('.add-meta-row');
        if (add) {
            addMetaRow(add.previousElementSibling);
            return;
        }
        const paste = target.closest('[data-import-paste]');
        if (paste) {
            pasteAndImport(paste.dataset.importPaste);
            return;
        }
        const apply = target.closest('[data-import-apply]');
        if (apply) {
            importDestination(apply.dataset.importApply);
            return;
        }
        const exportBtn = target.closest('.export-dest');
        if (exportBtn) {
            exportDestination(exportBtn);
            return;
        }
        const reveal = target.closest('.reveal-btn');
        if (reveal) revealValue(reveal);
    });

    let controller = null;

    function init() {
        const root = document.querySelector('main');
        if (root && root.dataset.destinationsReady) return;
        if (controller) controller.abort();
        controller = null;
        if (!root) return;
        root.dataset.destinationsReady = '1';
        controller = new AbortController();
        const signal = controller.signal;
        destTypes = readDestinationTypes();

        document.querySelectorAll('.dest-type-select').forEach(function(select) {
            select.addEventListener('change', function() { updateTypeFields(select.dataset.index); }, { signal: signal });
            updateTypeFields(select.dataset.index);
        });

        document.querySelectorAll('form[id^="create-dest-form-"]').forEach(function(form) {
            form.addEventListener('submit', function() {
                form.querySelectorAll('input[name="sensitive_keys"]').forEach(function(el) { el.remove(); });

                form.querySelectorAll('.metadata-row').forEach(function(row) {
                    const box = row.querySelector('.meta-sensitive');
                    const keyInput = row.querySelector('input[name="metadata_keys"]');
                    if (!box || !box.checked || !keyInput || keyInput.disabled || !keyInput.value.trim()) return;

                    const hidden = document.createElement('input');
                    hidden.type = 'hidden';
                    hidden.name = 'sensitive_keys';
                    hidden.value = keyInput.value.trim();
                    form.appendChild(hidden);
                });
            }, { signal: signal });
        });

        const metadataForm = document.getElementById('metadata-form');
        const addRowBtn = document.getElementById('add-row-btn');
        if (metadataForm && addRowBtn) {
            addRowBtn.addEventListener('click', function() {
                const container = document.getElementById('metadata-rows');
                const row = document.createElement('div');
                row.className = 'px-5 py-3 flex items-center gap-3 metadata-row';
                row.innerHTML = rowHtml(DETAIL_ROW_INPUT, 'Mark as a credential: the value is hidden after saving. This cannot be undone from the UI.');
                container.appendChild(row);
                row.querySelector('input').focus();
            }, { signal: signal });
        }

        if (metadataForm) {
            metadataForm.addEventListener('submit', function() {
                const form = metadataForm;
                form.querySelectorAll('input[name="sensitive_keys"], input[name="set_sensitive_keys"]')
                    .forEach(function(el) { el.remove(); });

                const promoted = [];
                form.querySelectorAll('.metadata-row').forEach(function(row) {
                    const box = row.querySelector('.meta-sensitive');
                    const keyInput = row.querySelector('input[name="metadata_keys"]');
                    if (!box || !box.checked || !keyInput || !keyInput.value.trim()) return;
                    promoted.push(keyInput.value.trim());
                });
                if (promoted.length === 0) return;

                let declared = [];
                try { declared = JSON.parse(form.dataset.declaredSensitive || '[]'); } catch (e) { declared = []; }

                const marker = document.createElement('input');
                marker.type = 'hidden';
                marker.name = 'set_sensitive_keys';
                marker.value = '1';
                form.appendChild(marker);

                Array.from(new Set(declared.concat(promoted))).forEach(function(key) {
                    const hidden = document.createElement('input');
                    hidden.type = 'hidden';
                    hidden.name = 'sensitive_keys';
                    hidden.value = key;
                    form.appendChild(hidden);
                });
            }, { signal: signal });
        }

        const list = document.getElementById('env-list');
        if (!list || !list.querySelector('.env-card[draggable="true"]')) return;

        let dragged = null;
        let originalIndex = -1;

        function indexOf(card) {
            return Array.from(list.querySelectorAll('.env-card')).indexOf(card);
        }

        function postOrder(orgName, csrf, envId, sortOrder) {
            const body = new URLSearchParams();
            body.set('_csrf', csrf);
            body.set('sort_order', String(sortOrder));
            return fetch('/orgs/' + encodeURIComponent(orgName) + '/destinations/environments/' + encodeURIComponent(envId) + '/order', {
                method: 'POST',
                headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
                body: body.toString(),
                credentials: 'same-origin',
            });
        }

        async function commitReorder(card) {
            const newIndex = indexOf(card);
            if (newIndex < 0 || newIndex === originalIndex) return;

            const cards = Array.from(list.querySelectorAll('.env-card'));
            const csrfInput = document.querySelector('input[name="_csrf"]');
            const csrf = csrfInput ? csrfInput.value : '';
            const orgName = list.dataset.org;

            try {
                for (let i = 0; i < cards.length; i++) {
                    const resp = await postOrder(orgName, csrf, cards[i].dataset.envId, i * 10);
                    if (!resp.ok && resp.status !== 0) {
                        console.error('env reorder failed for', cards[i].dataset.envId, 'status', resp.status);
                        break;
                    }
                }
            } catch (err) {
                console.error('env reorder error:', err);
            }
            window.location.reload();
        }

        list.addEventListener('dragover', function(e) {
            if (!dragged) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = 'move';

            const target = e.target.closest('.env-card');
            if (!target || target === dragged) return;

            const rect = target.getBoundingClientRect();
            const insertBefore = (e.clientY - rect.top) < rect.height / 2;
            if (insertBefore) {
                list.insertBefore(dragged, target);
            } else {
                list.insertBefore(dragged, target.nextElementSibling);
            }
        }, { signal: signal });

        list.addEventListener('drop', function(e) {
            if (dragged) e.preventDefault();
        }, { signal: signal });

        list.querySelectorAll('.env-card').forEach(function(card) {
            card.addEventListener('dragstart', function(e) {
                dragged = card;
                originalIndex = indexOf(card);
                card.classList.add('opacity-50');
                e.dataTransfer.effectAllowed = 'move';
                try { e.dataTransfer.setData('text/plain', card.dataset.envId); } catch (_) {}
            }, { signal: signal });

            card.addEventListener('dragend', function() {
                card.classList.remove('opacity-50');
                const moved = dragged;
                dragged = null;
                if (!moved) return;
                commitReorder(moved);
            }, { signal: signal });
        });
    }

    init();
    document.addEventListener('sedx:load', init);
})();
