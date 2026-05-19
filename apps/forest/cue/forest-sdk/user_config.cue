// Forest user-global config schema (TASKS/018-global-tools.md §1a.4).
//
// This is the schema for `~/.config/forest/forest.cue`, the file written by
// `forest global add` / `forest global update` / etc.

package sdk

#UserConfig: {
	// Open struct of arbitrary string→string user-set keys (matches the
	// pre-spec `[user]` table semantics). Set via `forest global set`.
	user: [string]: string

	// Per-tool pins. Key is "<org>/<name>"; value carries the resolved version.
	dependencies: [string]: {
		version: string & =~#"^\d+\.\d+\.\d+"#

		// Optional client-side shim alias. If unset, the shim name comes from
		// the component manifest's `#Tool.name` (which itself defaults to the
		// component name).
		shim_name?: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}

	// Org-catalogue subscriptions. Key is the organisation name.
	org_catalog: [string]: {
		enabled: bool | *true
		banned:  [...string] | *[]

		// Optional per-tool pins inside this catalogue subscription.
		// Key is the upstream `tool.name`; value is the version to pin.
		pins: [string]: string & =~#"^\d+\.\d+\.\d+"#

		// Optional alias map. Key is the upstream `tool.name`; value is the
		// local shim filename. Affects the shim file on disk; does NOT
		// change the qualified ref embedded in the shim body.
		aliases: [string]: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}
}
