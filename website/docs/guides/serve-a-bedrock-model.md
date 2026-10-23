---
title: Serve a Bedrock model
sidebar_position: 7
description: Declare an Amazon Bedrock model as a hosted model, sign its requests with the container credentials Amazon EKS Pod Identity provides, and know which request fields Bedrock does not take.
lede: A hosted model of kind bedrock is reached through llm's Bedrock client, every request translated and signed with AWS Signature Version 4 from the container credentials endpoint; no key file is involved.
source: crates/llm-gateway-cli/src/bedrock.rs, crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/translate.rs, crates/llm-gateway-cli/tests/bedrock.rs (a fixture Bedrock Runtime and credentials endpoint), spec/domains/upstream.yaml (HostedEndpoint) and spec/domains/deployment.yaml (HostedDeclaration); the Pod Identity section follows the AWS documentation and has not been run against AWS
---

# Serve a Bedrock model

A `[hosted.<alias>]` table with `kind = "bedrock"` declares a model Amazon Bedrock serves. The
gateway sends each request to Bedrock Runtime with llm's Bedrock client, signed with AWS
Signature Version 4 from the credentials of the container credentials endpoint. Clients call the
model by its alias on any wire, as they call any other model.

:::caution[Not verified against AWS]
The gateway's Bedrock path is tested against a fixture Bedrock Runtime and a fixture credentials
endpoint only. No request in this repository has reached AWS. The model id and the Pod Identity
setup below are examples to check against your account and the AWS documentation.
:::

## Declare the model

```toml
[hosted.haiku]
kind = "bedrock"
region = "eu-central-1"
model_id = "eu.anthropic.claude-haiku-5-5"
credential_source = "container"
context_window = 200000
```

`eu.anthropic.claude-haiku-5-5` is an example inference-profile id and is **unverified**: look
up the id your account offers in the Bedrock console or with `aws bedrock
list-inference-profiles` before you use it.

| Key | Default | Rule |
| --- | --- | --- |
| `kind` | `endpoint` | `bedrock` for a Bedrock model. |
| `region` | required | The AWS Region, two or more parts of `a-z` and `0-9` joined by `-`, at most 32 characters. |
| `model_id` | required | The model id, inference-profile id or ARN Bedrock is asked for, 1 to 2048 printable ASCII characters. |
| `credential_source` | required | `container`, the only source: the container credentials endpoint. |
| `api` | by the model id | `anthropic-invoke` (InvokeModel with an Anthropic Messages body) or `converse` (the Converse API). |
| `endpoint` | `https://bedrock-runtime.<region>.amazonaws.com` | An `https` origin, or an `http` origin on a loopback address, with no path, query, fragment or user information. |
| `billing_kind` | `metered` | `metered`, `subscription` or `self-hosted`. |
| `context_window` | absent | 4096 to 2000000 tokens. Without one, the gateway works with 200000. |
| `max_output_tokens` | the window | 1 to the window. The most output a turn may ask for; a turn naming none is given half the window, at most 16384 and at most this. The inventory lists it. |
| `anthropic_beta` | empty | The `anthropic-beta` names a Messages client may send to this model, at most 32 distinct names of `A-Z a-z 0-9 - _ .`, each at most 128 bytes. `anthropic-invoke` only: refused on `converse`, an empty list too. See [Beta names](#beta-names). |
| `tool_calling` | `parsed` | `parsed` or `absent`. |

The default `api` is `anthropic-invoke` when the model id is Anthropic's: its first dotted part
is `anthropic`, or its second after a geography prefix such as `eu.` or `us.`, or an ARN's last
path segment reads that way. Any other id defaults to `converse`.

An endpoint's keys (`wires`, `base_url`, `auth_kind`, `credential_file`, `credential_header`,
`upstream_model`, `reasoning_effort`, `headers`) are refused on a Bedrock model, and a Bedrock
model's keys are refused on an endpoint. Each refusal is `config:value` naming the key, never the
value.

The route inventory lists the model as one target, `<alias>.bedrock`: protocol `messages` for
`anthropic-invoke` or `bedrock-converse` for `converse`, provider `bedrock`, account `container`,
`auth_kind` `aws-sigv4`. It never shows the region, the model id or the endpoint.

## The container credentials

A document with a Bedrock model needs two variables in the gateway's environment, both of which
Amazon EKS Pod Identity and Amazon ECS task roles set:

| Variable | Rule |
| --- | --- |
| `AWS_CONTAINER_CREDENTIALS_FULL_URI` | `https`, or `http` to a loopback address or to `169.254.170.2`, `169.254.170.23` or `[fd00:ec2::23]`. |
| `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` | A file the trusted-file reader accepts once its symbolic links are resolved, to a file inside the token path's directory: at most 16386 bytes, owned by you or root, not group- or world-writable (readable is fine), holding one line of printable ASCII. |

Either one unset or empty refuses the start as `container-credentials:unset`, a URI outside the
rule as `container-credentials:uri`, and a token file as `container-token:<rule>`. No message
quotes the URI or the token. The token file is read at start and again on every credential
fetch, its links resolved each time, so a rotated token is used without a restart. This is what
a Kubernetes projected volume needs: the token path links to `..data/<name>`, `..data` links to a
timestamped directory, and the kubelet swaps that link when it rotates the token. That layout
is taken from the AWS and Kubernetes documentation and is unverified here against a cluster. A document without a Bedrock model reads
neither variable.

## What is sent and what is dropped

Every request to a Bedrock model is translated, the Messages wire included: the gateway decodes
it with llm's decoder for the client's wire and sends llm's neutral turn to Bedrock. Chat tool
turns stream their tool-call deltas or answer whole, a one-token warm-up probe
(`max_tokens: 1`) is answered, and Claude Code's `claude -p` turn reaches an Anthropic model
through InvokeModel with its signed thinking carried back on the next turn.

Bedrock does not take some fields a client sends. The gateway drops these and logs the field's
name at `info`, never its value, and never prompt or answer text:

| Field | Dropped on |
| --- | --- |
| A reasoning effort (`reasoning_effort`, `reasoning.effort`, `output_config.effort`) | every Bedrock model |
| Parallel tool calls (`parallel_tool_calls`, `tool_choice.disable_parallel_tool_use`) | `converse` |
| Thinking (`thinking`, `reasoning`) | a model id that is not Anthropic's |

A request without an output limit is sent half the context window, at most 16384 tokens. A
`temperature` above 1 on `anthropic-invoke` and `output_config.format` are still refused, not
dropped. Before anything is sent, the turn is checked with llm's own Bedrock check
(`llm_bedrock::check_request`), so a turn the Bedrock client would refuse is refused with no
request to AWS.

### Beta names {#beta-names}

No client header reaches Bedrock. Bedrock refuses a beta name it does not support, and Claude
Code sends several, so a beta reaches Bedrock only when the model's `anthropic_beta` list names
it:

```toml
[hosted.haiku]
kind = "bedrock"
region = "eu-central-1"
model_id = "eu.anthropic.claude-haiku-5-5"
credential_source = "container"
anthropic_beta = ["context-1m-2025-08-07"]
```

The beta name above is an example and is unverified against Bedrock.

Only for a Messages client of an `anthropic-invoke` model whose list is not empty does llm's
decoder read the `anthropic-beta` header, one line, since a repeated header is refused. A header that is not visible ASCII, names a beta outside the name rule,
or names more than 32 is refused `translation-invalid` (400) before anything is sent. The names
on the list go into the InvokeModel body's `anthropic_beta`, in the client's order; every other
name is dropped and logged at `info` with `field = "anthropic-beta"` and the name as `beta`. A
model without a list, a `converse` model, and a client on another wire send no beta: the header
is not read, so a malformed one is not refused, and its name alone is logged as dropped.

## Amazon EKS Pod Identity

:::caution[Unverified]
This section follows the AWS documentation for EKS Pod Identity and has not been run against a
cluster. Check each step against the current AWS documentation.
:::

1. Install the Amazon EKS Pod Identity Agent add-on on the cluster.
2. Create an IAM role whose trust policy lets the principal `pods.eks.amazonaws.com` perform
   `sts:AssumeRole` and `sts:TagSession`, and grant it `bedrock:InvokeModel` and
   `bedrock:InvokeModelWithResponseStream` on the foundation models and inference profiles the
   gateway calls. An inference profile spans several Regions, so the policy names the foundation
   model in each of them.
3. Associate the role with the gateway's service account:
   `aws eks create-pod-identity-association --cluster-name <cluster> --namespace <namespace>
   --service-account <service account> --role-arn <role ARN>`.
4. Run the gateway under that service account. The Pod Identity webhook sets
   `AWS_CONTAINER_CREDENTIALS_FULL_URI` (`http://169.254.170.23/v1/credentials`) and
   `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`, and mounts the token as its own projected volume,
   so the pod does not need the default service account token:

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: llm-gateway
automountServiceAccountToken: false
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: llm-gateway
spec:
  template:
    spec:
      serviceAccountName: llm-gateway
      automountServiceAccountToken: false
```

The rest of the Deployment is the one [Run on Kubernetes](run-on-kubernetes.md) describes. The
gateway must also reach `169.254.170.23` on port 80 and `bedrock-runtime.<region>.amazonaws.com`
on port 443 if a network policy limits its egress.
