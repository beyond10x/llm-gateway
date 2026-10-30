//! The credential-namespace manifests (`deploy/kubernetes/credential-namespace/`) and the guide
//! that embeds them: the files are what the page shows, the policy admits one namespace on one
//! port, the Deployment carries the provider key only as a mounted file, and the gateway starts on
//! the `ConfigMap`'s document and serves model calls with no credential. Only loopback sockets and
//! files under `CARGO_TARGET_TMPDIR` are used; no cluster is needed.

use rustix::process::{getegid, geteuid};
use serde_yaml::{Deserializer, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

const GUIDE: &str = include_str!("../../../website/docs/guides/run-in-a-credential-namespace.md");
const MANIFESTS: [(&str, &str); 3] = [
    (
        "00-namespace.yaml",
        include_str!("../../../deploy/kubernetes/credential-namespace/00-namespace.yaml"),
    ),
    (
        "10-network-policy.yaml",
        include_str!("../../../deploy/kubernetes/credential-namespace/10-network-policy.yaml"),
    ),
    (
        "20-gateway.yaml",
        include_str!("../../../deploy/kubernetes/credential-namespace/20-gateway.yaml"),
    ),
];
const NAMESPACE: &str = "agent-credentials";
const WARNING: &str = "b10x-llm-gateway: warning ingress auth is off (ingress_auth = \"none\"): model calls are served without a credential";
const LISTENING: &str = "b10x-llm-gateway: listening on ";

fn manifest(name: &str) -> &'static str {
    MANIFESTS
        .iter()
        .find_map(|(file, text)| (*file == name).then_some(*text))
        .unwrap_or_else(|| panic!("no manifest {name}"))
}

fn documents() -> Vec<(&'static str, Value)> {
    MANIFESTS
        .iter()
        .flat_map(|(file, text)| {
            Deserializer::from_str(text).map(move |document| {
                (
                    *file,
                    serde::Deserialize::deserialize(document)
                        .unwrap_or_else(|error| panic!("{file} is not YAML: {error}")),
                )
            })
        })
        .collect()
}

fn document(kind: &str) -> Value {
    let found: Vec<Value> = documents()
        .into_iter()
        .map(|(_, value)| value)
        .filter(|value| value["kind"] == kind)
        .collect();
    assert_eq!(found.len(), 1, "exactly one {kind} in the manifests");
    found.into_iter().next().unwrap()
}

fn texts(value: &Value) -> Vec<String> {
    value
        .as_sequence()
        .unwrap_or_else(|| panic!("{value:?} is not a list"))
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn no_manifest_carries_a_comment() {
    for (file, text) in MANIFESTS {
        for line in text.lines() {
            assert!(
                !line.trim_start().starts_with('#'),
                "{file} has a comment line: {line:?}"
            );
        }
    }
}

#[test]
fn every_object_but_the_namespace_lives_in_the_credential_namespace() {
    let mut seen = Vec::new();
    for (file, value) in documents() {
        let kind = value["kind"].as_str().unwrap().to_owned();
        let name = value["metadata"]["name"].as_str().unwrap().to_owned();
        if kind == "Namespace" {
            assert_eq!(name, NAMESPACE, "{file}");
        } else {
            assert_eq!(
                value["metadata"]["namespace"].as_str(),
                Some(NAMESPACE),
                "{file}: {kind} {name}"
            );
        }
        seen.push(kind);
    }
    seen.sort();
    assert_eq!(
        seen,
        [
            "ConfigMap",
            "Deployment",
            "Namespace",
            "NetworkPolicy",
            "Secret",
            "Service"
        ]
    );
}

#[test]
fn the_policy_admits_the_agents_namespace_on_8080_and_nothing_else() {
    let policy = document("NetworkPolicy");
    let spec = &policy["spec"];
    assert_eq!(texts(&spec["policyTypes"]), ["Ingress"]);
    assert!(
        spec.get("egress").is_none(),
        "the gateway reaches the provider: no egress rule"
    );
    let rules = spec["ingress"].as_sequence().unwrap();
    assert_eq!(rules.len(), 1, "one ingress rule is the whole list");
    let peers = rules[0]["from"].as_sequence().unwrap();
    assert_eq!(peers.len(), 1, "one peer");
    let peer = peers[0].as_mapping().unwrap();
    assert_eq!(peer.len(), 1, "a namespace selector alone: {peer:?}");
    let selector = &peers[0]["namespaceSelector"];
    assert_eq!(selector.as_mapping().unwrap().len(), 1, "{selector:?}");
    let labels = selector["matchLabels"].as_mapping().unwrap();
    assert_eq!(labels.len(), 1, "{labels:?}");
    assert_eq!(
        labels["kubernetes.io/metadata.name"].as_str(),
        Some("agents")
    );
    let ports = rules[0]["ports"].as_sequence().unwrap();
    assert_eq!(ports.len(), 1, "one port");
    assert_eq!(ports[0]["port"].as_u64(), Some(8080));
    assert_eq!(ports[0]["protocol"].as_str(), Some("TCP"));
}

#[test]
fn the_policy_service_and_deployment_name_the_same_pods_and_port() {
    let policy = document("NetworkPolicy");
    let service = document("Service");
    let deployment = document("Deployment");
    let labels = &deployment["spec"]["template"]["metadata"]["labels"];
    assert_eq!(&deployment["spec"]["selector"]["matchLabels"], labels);
    assert_eq!(&policy["spec"]["podSelector"]["matchLabels"], labels);
    assert_eq!(&service["spec"]["selector"], labels);
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(container["ports"][0]["name"].as_str(), Some("http"));
    assert_eq!(container["ports"][0]["containerPort"].as_u64(), Some(8080));
    assert_eq!(service["spec"]["ports"][0]["port"].as_u64(), Some(8080));
    assert_eq!(
        service["spec"]["ports"][0]["targetPort"].as_str(),
        Some("http")
    );
    assert_eq!(service["spec"]["type"].as_str(), Some("ClusterIP"));
}

#[test]
fn the_provider_key_reaches_the_gateway_only_as_a_file() {
    let deployment = document("Deployment");
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"].as_bool(), Some(false));
    for container in pod["containers"]
        .as_sequence()
        .unwrap()
        .iter()
        .chain(pod["initContainers"].as_sequence().unwrap())
    {
        let name = container["name"].as_str().unwrap();
        assert!(container.get("env").is_none(), "{name} declares env");
        assert!(
            container.get("envFrom").is_none(),
            "{name} declares envFrom"
        );
    }
    assert!(
        manifest("20-gateway.yaml").contains("defaultMode: 0400"),
        "the Secret volume is mounted 0400"
    );
    let secret_volume = pod["volumes"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|volume| volume["name"] == "secrets-in")
        .unwrap();
    assert_eq!(
        secret_volume["secret"]["secretName"],
        document("Secret")["metadata"]["name"]
    );
    let configmap = document("ConfigMap");
    let toml: toml::Table = configmap["data"]["gateway.toml"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(toml["listen"].as_str(), Some("0.0.0.0:8080"));
    assert_eq!(toml["ingress_auth"].as_str(), Some("none"));
    assert!(toml.get("owner_secret_file").is_none());
    let hosted = toml["hosted"]["claude"].as_table().unwrap();
    assert_eq!(
        hosted["credential_file"].as_str(),
        Some("/run/llm-gateway/provider-key")
    );
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "credential-namespace-{name}-{}",
        std::process::id()
    ));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn run_init(script: &str) -> Output {
    let mut command = if geteuid().is_root() {
        let mut command = Command::new("setpriv");
        command.args(["--bounding-set", "-all,+chown", "--", "sh", "-c", script]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    };
    command.stdin(Stdio::null()).output().unwrap()
}

struct Gateway {
    child: Child,
    address: SocketAddr,
    lines: Vec<String>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn serve(dir: &Path) -> Gateway {
    let input = dir.join("in");
    let output = dir.join("run");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(&output).unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o2777)).unwrap();
    let key = input.join("provider-key");
    fs::write(&key, b"provider-key-0123456789abcdefghijklmnop\n").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o440)).unwrap();
    let deployment = document("Deployment");
    let script = deployment["spec"]["template"]["spec"]["initContainers"][0]["command"][2]
        .as_str()
        .unwrap()
        .replace("\"/in/", &format!("\"{}/", input.display()))
        .replace("\"/run/llm-gateway/", &format!("\"{}/", output.display()))
        .replace(
            "65532:65532",
            &format!("{}:{}", geteuid().as_raw(), getegid().as_raw()),
        );
    let init = run_init(&script);
    assert!(init.status.success(), "the init script failed: {init:?}");
    let copy = fs::metadata(output.join("provider-key")).unwrap();
    assert_eq!(copy.mode() & 0o7777, 0o400, "the copy is 0400");
    let configmap = document("ConfigMap");
    let config = dir.join("gateway.toml");
    fs::write(
        &config,
        configmap["data"]["gateway.toml"]
            .as_str()
            .unwrap()
            .replace("\"0.0.0.0:8080\"", "\"127.0.0.1:0\"")
            .replace("\"/run/llm-gateway/", &format!("\"{}/", output.display())),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut gateway = Gateway {
        child,
        address: SocketAddr::from(([127, 0, 0, 1], 0)),
        lines: Vec::new(),
    };
    let mut reader = BufReader::new(gateway.child.stderr.take().unwrap());
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).unwrap();
        assert!(
            read > 0,
            "the gateway exited before it listened: {:?}",
            gateway.lines
        );
        let line = line.trim_end().to_owned();
        gateway.lines.push(line.clone());
        if let Some(address) = line.strip_prefix(LISTENING) {
            gateway.address = address.parse().unwrap();
            return gateway;
        }
    }
}

fn get(address: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: gateway\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let status = answer
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("no status in {answer:?}"))
        .parse()
        .unwrap();
    (status, answer)
}

#[test]
fn the_gateway_starts_on_the_configmap_and_takes_a_model_listing_with_no_credential() {
    let dir = scratch("serve");
    let gateway = serve(&dir);
    assert_eq!(gateway.lines[0], WARNING, "{:?}", gateway.lines);
    let (ready, _) = get(gateway.address, "/ready");
    assert_eq!(ready, 200);
    let (models, body) = get(gateway.address, "/v1/models");
    assert_eq!(models, 200, "{body}");
    assert!(body.contains("\"claude\""), "the model is listed: {body}");
    let (routes, body) = get(gateway.address, "/v1/routes");
    assert_eq!(routes, 401, "inspection still asks for the owner: {body}");
    assert!(body.contains("credential-absent"), "{body}");
    assert!(
        !body.contains("provider-key"),
        "no key material in an answer: {body}"
    );
}

#[test]
fn the_guide_pastes_the_start_lines_the_binary_prints() {
    let dir = scratch("pasted");
    let gateway = serve(&dir);
    let address = gateway.address.to_string();
    let printed: Vec<String> = gateway
        .lines
        .iter()
        .map(|line| line.replace(&address, "0.0.0.0:8080"))
        .collect();
    assert_eq!(printed.len(), 2, "{printed:?}");
    for line in &printed {
        assert!(
            GUIDE.lines().any(|candidate| candidate == line),
            "the guide must paste {line:?} on a line of its own"
        );
    }
}

fn fence(title: &str) -> String {
    let opener = format!("```yaml title=\"{title}\"");
    let mut lines = GUIDE.lines().skip_while(|line| *line != opener).skip(1);
    let mut text = String::new();
    loop {
        let line = lines
            .next()
            .unwrap_or_else(|| panic!("the guide's {title} block never closes"));
        if line == "```" {
            return text;
        }
        text.push_str(line);
        text.push('\n');
    }
}

#[test]
fn the_guide_shows_each_manifest_exactly_as_the_file_has_it() {
    for (file, text) in MANIFESTS {
        assert_eq!(
            fence(file),
            text,
            "the guide's {file} block differs from deploy/kubernetes/credential-namespace/{file}"
        );
    }
}

#[test]
fn the_guide_is_in_the_sidebar_and_the_index() {
    let sidebar = include_str!("../../../website/sidebars.ts");
    assert!(sidebar.contains("'guides/run-in-a-credential-namespace'"));
    let index = include_str!("../../../website/docs/index.md");
    assert!(index.contains("(guides/run-in-a-credential-namespace.md)"));
}
