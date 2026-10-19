package sdk

#UserConfig: {
	user: [string]: string

	dependencies: [string]: {
		version: string & =~#"^\d+\.\d+\.\d+"#

		pinned?: bool | *false

		shim_name?: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}

	org_catalog: [string]: {
		enabled: bool | *true
		banned: [...string] | *[]

		pins: [string]: string & =~#"^\d+\.\d+\.\d+"#

		aliases: [string]: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"
	}
}
