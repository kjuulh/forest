fn test_fixture() -> &'static str {
    r##"
{
    "openapi": "3.0.0",
    "info": {
        "title": "let S = #Spec\nlet C = #Commands\nlet H = #Hooks",
        "version": "no version"
    },
    "paths": {},
    "components": {
        "schemas": {
            "CPU": {
                "type": "integer",
                "enum": [
                    256,
                    512,
                    1024,
                    2048,
                    4096
                ]
            },
            "Commands": {
                "description": "--- Commands ---",
                "type": "object",
                "properties": {
                    "prepare": {
                        "type": "object",
                        "required": [
                            "description",
                            "input",
                            "output"
                        ],
                        "properties": {
                            "description": {
                                "type": "string",
                                "enum": [
                                    "Generate ECS task definition and service manifests"
                                ]
                            },
                            "input": {
                                "type": "object"
                            },
                            "output": {
                                "type": "object"
                            }
                        }
                    },
                    "status": {
                        "type": "object",
                        "required": [
                            "description",
                            "input",
                            "output"
                        ],
                        "properties": {
                            "description": {
                                "type": "string",
                                "enum": [
                                    "Check service health and running count"
                                ]
                            },
                            "input": {
                                "type": "object"
                            },
                            "output": {
                                "type": "object",
                                "required": [
                                    "running",
                                    "desired",
                                    "healthy"
                                ],
                                "properties": {
                                    "running": {
                                        "type": "integer"
                                    },
                                    "desired": {
                                        "type": "integer"
                                    },
                                    "healthy": {
                                        "type": "boolean"
                                    }
                                }
                            }
                        }
                    }
                },
                "allOf": [
                    {
                        "$ref": "#/components/schemas/ForestCommands"
                    },
                    {
                        "required": [
                            "prepare",
                            "status"
                        ]
                    }
                ]
            },
            "Component": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "enum": [
                            "ecs-service"
                        ]
                    },
                    "org": {
                        "type": "string",
                        "enum": [
                            "forest-contrib"
                        ]
                    },
                    "version": {
                        "type": "string",
                        "enum": [
                            "0.1.0"
                        ]
                    }
                },
                "allOf": [
                    {
                        "$ref": "#/components/schemas/ForestComponent"
                    },
                    {
                        "required": [
                            "name",
                            "org",
                            "version"
                        ]
                    }
                ]
            },
            "ForestCommand": {
                "type": "object",
                "required": [
                    "description",
                    "input",
                    "output"
                ],
                "properties": {
                    "description": {
                        "type": "string"
                    },
                    "input": {
                        "type": "object"
                    },
                    "output": {
                        "type": "object"
                    }
                }
            },
            "ForestCommands": {
                "type": "object",
                "additionalProperties": {
                    "$ref": "#/components/schemas/ForestCommand"
                }
            },
            "ForestComponent": {
                "type": "object",
                "required": [
                    "name",
                    "org",
                    "version"
                ],
                "properties": {
                    "name": {
                        "type": "string",
                        "pattern": "^[a-z][a-z0-9-]*$"
                    },
                    "org": {
                        "type": "string"
                    },
                    "version": {
                        "type": "string",
                        "pattern": "^\\d\\.\\d\\.\\d"
                    }
                }
            },
            "ForestHook": {
                "type": "object"
            },
            "ForestHooks": {
                "type": "object",
                "additionalProperties": {
                    "$ref": "#/components/schemas/ForestHook"
                }
            },
            "ForestSpec": {
                "type": "object"
            },
            "HealthCheck": {
                "type": "object",
                "required": [
                    "path",
                    "interval",
                    "timeout",
                    "retries"
                ],
                "properties": {
                    "path": {
                        "type": "string",
                        "default": "/health"
                    },
                    "interval": {
                        "type": "integer",
                        "minimum": 5,
                        "maximum": 300,
                        "default": 30
                    },
                    "timeout": {
                        "type": "integer",
                        "minimum": 2,
                        "maximum": 60,
                        "default": 5
                    },
                    "retries": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "default": 3
                    }
                }
            },
            "Hooks": {
                "description": "--- Lifecycle hooks ---",
                "type": "object",
                "properties": {
                    "forest/deployment": {
                        "type": "object",
                        "properties": {
                            "prepare": {
                                "$ref": "#/components/schemas/Commands.prepare"
                            },
                            "release": {
                                "type": "object",
                                "required": [
                                    "description",
                                    "input",
                                    "output"
                                ],
                                "properties": {
                                    "description": {
                                        "type": "string",
                                        "enum": [
                                            "Deploy to ECS"
                                        ]
                                    },
                                    "input": {
                                        "type": "object",
                                        "required": [
                                            "release_id"
                                        ],
                                        "properties": {
                                            "release_id": {
                                                "type": "string"
                                            }
                                        }
                                    },
                                    "output": {
                                        "type": "object"
                                    }
                                }
                            },
                            "rollback": {
                                "type": "object",
                                "required": [
                                    "description",
                                    "input"
                                ],
                                "properties": {
                                    "description": {
                                        "type": "string",
                                        "enum": [
                                            "Roll back to previous task definition"
                                        ]
                                    },
                                    "input": {
                                        "type": "object",
                                        "required": [
                                            "name",
                                            "environment",
                                            "release_id"
                                        ],
                                        "properties": {
                                            "name": {
                                                "$ref": "#/components/schemas/Spec.name"
                                            },
                                            "environment": {
                                                "$ref": "#/components/schemas/Spec.environment"
                                            },
                                            "release_id": {
                                                "type": "string"
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "allOf": [
                            {
                                "$ref": "#/components/schemas/ForestHook"
                            },
                            {
                                "required": [
                                    "prepare",
                                    "release",
                                    "rollback"
                                ]
                            }
                        ]
                    }
                },
                "allOf": [
                    {
                        "$ref": "#/components/schemas/ForestHooks"
                    },
                    {
                        "required": [
                            "forest/deployment"
                        ]
                    }
                ]
            },
            "Memory": {
                "type": "integer",
                "enum": [
                    512,
                    1024,
                    2048,
                    4096,
                    8192
                ]
            },
            "Port": {
                "description": "--- Type definitions ---",
                "type": "object",
                "required": [
                    "name",
                    "port",
                    "protocol",
                    "external"
                ],
                "properties": {
                    "name": {
                        "type": "string"
                    },
                    "port": {
                        "type": "integer",
                        "minimum": 0,
                        "exclusiveMinimum": true,
                        "maximum": 65535
                    },
                    "protocol": {
                        "type": "string",
                        "enum": [
                            "tcp",
                            "udp"
                        ],
                        "default": "tcp"
                    },
                    "external": {
                        "type": "boolean",
                        "default": false
                    }
                }
            },
            "Spec": {
                "description": "--- Input spec: what callers must/can provide ---",
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "pattern": "^[a-z][a-z0-9-]*$"
                    },
                    "image": {
                        "type": "string"
                    },
                    "ports": {
                        "type": "array",
                        "items": {
                            "$ref": "#/components/schemas/Port"
                        }
                    },
                    "cpu": {
                        "type": "integer",
                        "enum": [
                            256,
                            512,
                            1024,
                            2048,
                            4096
                        ],
                        "default": 256
                    },
                    "memory": {
                        "type": "integer",
                        "enum": [
                            512,
                            1024,
                            2048,
                            4096,
                            8192
                        ],
                        "default": 512
                    },
                    "replicas": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "default": 1
                    },
                    "environment": {
                        "type": "string",
                        "enum": [
                            "dev",
                            "staging",
                            "prod"
                        ]
                    },
                    "health_check": {
                        "type": "object",
                        "default": {
                            "path": "/health",
                            "interval": 30
                        },
                        "oneOf": [
                            {
                                "$ref": "#/components/schemas/HealthCheck"
                            },
                            {
                                "allOf": [
                                    {
                                        "required": [
                                            "path",
                                            "interval"
                                        ],
                                        "properties": {
                                            "path": {
                                                "type": "string",
                                                "enum": [
                                                    "/health"
                                                ]
                                            },
                                            "interval": {
                                                "type": "integer",
                                                "enum": [
                                                    30
                                                ]
                                            }
                                        }
                                    },
                                    {
                                        "not": {
                                            "anyOf": [
                                                {
                                                    "$ref": "#/components/schemas/HealthCheck"
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        ]
                    }
                },
                "allOf": [
                    {
                        "$ref": "#/components/schemas/ForestSpec"
                    },
                    {
                        "required": [
                            "name",
                            "image",
                            "ports",
                            "cpu",
                            "memory",
                            "replicas",
                            "environment",
                            "health_check"
                        ]
                    }
                ]
            }
        }
    }
}
"##
}

#[test]
fn test_can_parse_openapi() -> anyhow::Result<()> {
    forest_sdk_codegen::openapi::parse(test_fixture())?;
    Ok(())
}

#[test]
fn test_can_lower_to_ir() -> anyhow::Result<()> {
    let doc = forest_sdk_codegen::openapi::parse(test_fixture())?;
    let module = forest_sdk_codegen::lower::lower(&doc)?;

    // Snapshot the IR debug output
    insta::assert_debug_snapshot!("ir_module", module);

    Ok(())
}

#[test]
fn test_full_codegen_pipeline() -> anyhow::Result<()> {
    let codegen = forest_sdk_codegen::Codegen {
        options: forest_sdk_codegen::CodegenOptions {
            destination: "/tmp/test".to_string(),
            language: forest_sdk_codegen::CodegenLanguage::Rust,
        },
    };

    let output = codegen.generate(test_fixture())?;

    // Snapshot the generated Rust code
    insta::assert_snapshot!("generated_rust", output);

    Ok(())
}
