//! forest#288: `destination create` names the projects a new destination is
//! added to without their asking.
//!
//! A project that declares no destinations for an environment is released to
//! every destination there of a kind its artifact renders. So a new
//! destination silently widens exactly those projects — and the create call is
//! the one moment someone is looking. These tests pin who is, and is not,
//! reported.

use std::collections::HashMap;

use forest_grpc_interface::{CreateDestinationRequest, DestinationType, ReleaseRequest};

use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

const FLUX: &str = "forest/flux@1";
const TERRAFORM: &str = "forest/terraform@1";

fn destination_type(spec: &str) -> DestinationType {
    let (organisation, rest) = spec.split_once('/').expect("org/name@version");
    let (name, version) = rest.split_once('@').expect("org/name@version");
    DestinationType {
        organisation: organisation.into(),
        name: name.into(),
        version: version.parse().expect("numeric"),
        description: String::new(),
        fields: vec![],
    }
}

fn flux_metadata() -> HashMap<String, String> {
    let local_path = format!("/tmp/forest-accept-test-{}", uuid::Uuid::now_v7());
    std::fs::create_dir_all(&local_path).expect("create local path");
    HashMap::from([
        ("cluster_name".into(), "test-cluster".into()),
        ("namespace".into(), "test-namespace".into()),
        ("local_path".into(), local_path),
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_a_destination_names_the_undeclared_projects_it_widens() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let quiet_env = format!("accept-quiet-{suffix}");

    // A flux-rendering project that has released into `env` without declaring
    // anything for it — the shape that fans out.
    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .an_environment(&quiet_env)
        .await
        .a_destination_of_type(&format!("existing-{suffix}"), &env, FLUX)
        .await
        .an_uploaded_artifact_declaring_types("some-other-env", &[(".*", FLUX)])
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    when.fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await?;

    let create = |name: String, environment: String, kind: &'static str| {
        let mut client = when.fixture().destinations();
        let token = token.clone();
        let org = org.clone();
        async move {
            client
                .create_destination(authed_request(
                    &token,
                    CreateDestinationRequest {
                        organisation: org,
                        name,
                        environment,
                        metadata: if kind == FLUX {
                            flux_metadata()
                        } else {
                            HashMap::new()
                        },
                        r#type: Some(destination_type(kind)),
                        sensitive_keys: vec![],
                    },
                ))
                .await
                .expect("create destination")
                .into_inner()
                .widened_projects
        }
    };

    // Of a kind the project renders: it will be released there next time.
    let widened = create(format!("second-flux-{suffix}"), env.clone(), FLUX).await;
    assert_eq!(
        widened,
        vec![format!("{org}/test-project")],
        "the undeclared flux project gains a new flux destination",
    );

    // Of a kind it never renders: the scheduler will not send it there, so
    // there is nothing to warn about.
    let widened = create(format!("tf-{suffix}"), env.clone(), TERRAFORM).await;
    assert!(
        widened.is_empty(),
        "a terraform destination does not widen a flux-only project: {widened:?}",
    );

    // An environment the project has never released into.
    let widened = create(format!("quiet-{suffix}"), quiet_env.clone(), FLUX).await;
    assert!(
        widened.is_empty(),
        "no history in the environment, nothing to report: {widened:?}",
    );

    Ok(())
}
