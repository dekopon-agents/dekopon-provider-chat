use dekopon_broker::{AttestorGrant, IdentityDirectory};
use dekopon_core::{CapabilityId, ExternalSubject, PrincipalId, ProviderId, RiskLevel};
use dekopon_policy::{PolicyContext, PolicyEngine, PolicyRequest, PolicyTarget, PolicyWorld};
use dekopon_provider_sdk::EffectKind;

fn broker_fixture() -> serde_yaml_ng::Value {
    serde_yaml_ng::from_str(include_str!("fixtures/broker.yaml")).expect("valid broker fixture")
}

#[test]
fn attested_mapped_subjects_pass_invocation_authority_gates() {
    let fixture = broker_fixture();
    let grant: AttestorGrant =
        serde_yaml_ng::from_value(fixture["identities"][0]["attestor"].clone())
            .expect("attestor grant");
    grant.validate().expect("valid grant");
    let directory = IdentityDirectory::new([
        (
            "slack.tfixture.ufixture".parse().expect("slack subject"),
            "test-user".parse().expect("principal"),
        ),
        (
            "discord.123456789012345678"
                .parse()
                .expect("discord subject"),
            "test-user".parse().expect("principal"),
        ),
    ])
    .expect("fixture identity mapping");
    let world = PolicyWorld::new(
        ["test-user".parse().expect("principal")],
        [
            (
                "chat.slack.history.read".parse().expect("slack action"),
                "chat".parse().expect("provider"),
            ),
            (
                "chat.discord.history.read".parse().expect("discord action"),
                "chat".parse().expect("provider"),
            ),
        ],
    )
    .expect("world");
    let engine =
        PolicyEngine::new(include_str!("fixtures/policies.cedar"), &world).expect("policy");
    for (subject, transport, action) in [
        (
            "slack.tfixture.ufixture",
            "slack.test",
            "chat.slack.history.read",
        ),
        (
            "discord.123456789012345678",
            "discord.test",
            "chat.discord.history.read",
        ),
    ] {
        let subject: ExternalSubject = subject.parse().expect("valid subject");
        assert!(
            grant.permits(&subject),
            "attestor must admit mapped {subject:?}"
        );
        let principal = directory.resolve(&subject).expect("mapped subject").clone();
        let decision = engine.authorize(PolicyRequest {
            principal,
            target: PolicyTarget::Capability {
                capability: action.parse().expect("action"),
                provider: "chat".parse().expect("provider"),
                effect: EffectKind::ReadOnly,
                risk: RiskLevel::Low,
            },
            context: PolicyContext {
                via: Some("dekopon-gatewayd".into()),
                agent: Some("fixture-agent".into()),
                transport: Some(transport.into()),
                ..Default::default()
            },
        });
        assert!(
            decision.allowed,
            "mapped attested {subject:?} may invoke {action}: {decision:?}"
        );
    }
    let foreign: ExternalSubject = "discord.987654321098765432"
        .parse()
        .expect("foreign subject");
    assert!(
        !grant.permits(&foreign),
        "unmapped namespace must not become an attestation"
    );
    assert!(directory.resolve(&foreign).is_none());
}

#[test]
fn cedar_allows_only_the_matching_transport_and_read_action() {
    let fixture = broker_fixture();
    let principal: PrincipalId = "test-user".parse().expect("principal");
    let provider: ProviderId = "chat".parse().expect("provider");
    let world = PolicyWorld::new(
        [principal.clone()],
        [
            (
                "chat.slack.history.read".parse().expect("action"),
                provider.clone(),
            ),
            (
                "chat.discord.history.read".parse().expect("action"),
                provider.clone(),
            ),
            (
                "chat.send".parse().expect("future post action"),
                provider.clone(),
            ),
        ],
    )
    .expect("policy world");
    let engine =
        PolicyEngine::new(include_str!("fixtures/policies.cedar"), &world).expect("broker policy");
    for (action, transport) in [
        ("chat.slack.history.read", "slack.test"),
        ("chat.discord.history.read", "discord.test"),
    ] {
        for (current_transport, allowed) in [
            (transport, true),
            (
                if transport == "slack.test" {
                    "discord.test"
                } else {
                    "slack.test"
                },
                false,
            ),
            ("other", false),
        ] {
            let capability: CapabilityId = action.parse().expect("action");
            let decision = engine.authorize(PolicyRequest {
                principal: principal.clone(),
                target: PolicyTarget::Capability {
                    capability,
                    provider: provider.clone(),
                    effect: EffectKind::ReadOnly,
                    risk: RiskLevel::Low,
                },
                context: PolicyContext {
                    via: Some("dekopon-gatewayd".into()),
                    agent: Some("fixture-agent".into()),
                    transport: Some(current_transport.into()),
                    ..Default::default()
                },
            });
            assert_eq!(
                decision.allowed, allowed,
                "{action} on {current_transport}: {decision:?}"
            );
        }
    }
    let posted = engine.authorize(PolicyRequest {
        principal,
        target: PolicyTarget::Capability {
            capability: "chat.send".parse().expect("post"),
            provider,
            effect: EffectKind::ExternalWrite,
            risk: RiskLevel::High,
        },
        context: PolicyContext {
            via: Some("dekopon-gatewayd".into()),
            agent: Some("fixture-agent".into()),
            transport: Some("slack.test".into()),
            ..Default::default()
        },
    });
    assert!(!posted.allowed, "read grant cannot authorize a future post");
    assert_eq!(
        fixture["capabilities"]["chat"]["capabilities"]
            .as_mapping()
            .expect("capabilities")
            .len(),
        2
    );
}
