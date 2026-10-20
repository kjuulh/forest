package sdk

#UserConfig: {
	user: [string]: string

	dependencies: [string]: {
		version: string & =~#"^\d+\.\d+\.\d+"#

		pinned?: bool | *false

		// Follow this tool's prereleases: `forest global update` moves it to
		// the highest SemVer version, prerelease or stable. Set by `forest
		// global add <org>/<name> --prerelease`; re-running `add` without the
		// flag clears it.
		prerelease?: bool | *false

		shim_name?: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}

	org_catalog: [string]: {
		enabled: bool | *true
		banned: [...string] | *[]

		pins: [string]: string & =~#"^\d+\.\d+\.\d+"#

		aliases: [string]: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}
}
