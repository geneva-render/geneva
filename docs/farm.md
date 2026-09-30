# Running a farm

`geneva farm` splits a render into parts and hands them to `geneva
worker` processes that connect to it. The commands and their options
are in [cli.md](cli.md#geneva-farm-timeline--o-file); this page is about
where the workers come from.

A worker needs three things: the same geneva build as the farm (and
x264 on both or on neither), a way to reach the farm's address, and the
token. It needs no copy of the assets; the farm sends them.

Every example below uses `--launch`, which the farm runs once per part
unless `--launch-count` says otherwise, with `GENEVA_FARM_URL`,
`GENEVA_FARM_TOKEN` and `GENEVA_FARM_WORKER` (0, 1, ...) set. Each worker
uses all the cores it is given and keeps asking for parts until there
are none, so start about as many as there are machines, not one per
part. Workers can also be started by hand, or by anything else, with
the command the farm prints.

The token is passed on command lines and in environment variables, so
anyone who can list processes or read the task definition on those
machines can see it. It is only good for the one farm.

## Reaching the farm

The farm listens on every interface and prints the address of the one
the route out of the machine uses. That is right on a local network.
When the workers reach the farm some other way, give that way with
`--url`, and a fixed port with `--listen`:

```sh
geneva farm talk.json -o out.mp4 --listen 0.0.0.0:8700 \
  --url http://203.0.113.7:8700 --launch '...'
```

Traffic is plain HTTP. Workers outside a network you trust should reach
the farm through a tunnel or a proxy that adds TLS, and `--url` is then
the proxy's address.

## Over SSH

Each machine needs the same geneva installed. With one
host name per line in `hosts.txt`:

```sh
geneva farm talk.json -o out.mp4 --launch-count "$(wc -l < hosts.txt)" \
  --launch 'ssh "$(sed -n "$((GENEVA_FARM_WORKER + 1))p" hosts.txt)" \
    geneva worker --connect "$GENEVA_FARM_URL" --token "$GENEVA_FARM_TOKEN"'
```

## The image

The `Dockerfile` at the top of the repository builds an image with
geneva, x264 and the DejaVu fonts on Ubuntu 22.04. It compiles the media
libraries and geneva from the checkout, which takes a while the first
time.

```sh
docker build -t geneva .
docker buildx build --platform linux/arm64 -t geneva:arm64 .   # for Graviton and arm64 Lambda
```

With no arguments the container runs `geneva worker`, which reads
`GENEVA_FARM_URL` and `GENEVA_FARM_TOKEN`. With arguments it runs geneva
with them, so `docker run -v "$PWD:/w" -w /w geneva render talk.json -o
out.mp4` works too.

Text in a font the timeline does not ship comes from the fonts the
machine has, and the image has DejaVu only. When the farm's machine has
a family the timeline names and the image does not, the workers see
that their load differs from the farm's and refuse to render. Ship the
fonts with the timeline to avoid that.

## Docker

On this machine, or on another through `DOCKER_HOST=ssh://box2`:

```sh
geneva farm talk.json -o out.mp4 --local 0 --launch-count 2 \
  --launch 'docker run --rm -e GENEVA_FARM_URL -e GENEVA_FARM_TOKEN geneva'
```

A container on the default bridge network reaches the host at the
address the farm prints.

## Kubernetes

One pod per worker, from an image the cluster can pull:

```sh
geneva farm talk.json -o out.mp4 --local 0 --launch-count 8 \
  --listen 0.0.0.0:8700 --url http://farm.example.internal:8700 \
  --launch 'kubectl run "geneva-worker-$GENEVA_FARM_WORKER-$$" \
    --image=registry.example.com/geneva --restart=Never --labels=app=geneva-worker \
    --env=GENEVA_FARM_URL="$GENEVA_FARM_URL" \
    --env=GENEVA_FARM_TOKEN="$GENEVA_FARM_TOKEN"'
```

Pods that end are left for their logs; `kubectl delete pod -l
app=geneva-worker` removes them. `GET /metrics` on the farm reports the
parts waiting, for an autoscaler that adds workers.

## ECS on Fargate

With a task definition `geneva-worker` whose container, named `worker`,
runs the image:

```sh
geneva farm talk.json -o out.mp4 --local 0 --launch-count 8 \
  --listen 0.0.0.0:8700 --url http://203.0.113.7:8700 \
  --launch 'aws ecs run-task --cluster render --task-definition geneva-worker \
    --launch-type FARGATE \
    --network-configuration "awsvpcConfiguration={subnets=[subnet-0abc],assignPublicIp=ENABLED}" \
    --overrides "{\"containerOverrides\":[{\"name\":\"worker\",\"environment\":[
      {\"name\":\"GENEVA_FARM_URL\",\"value\":\"$GENEVA_FARM_URL\"},
      {\"name\":\"GENEVA_FARM_TOKEN\",\"value\":\"$GENEVA_FARM_TOKEN\"}]}]}" >/dev/null'
```

## AWS Lambda

The same image runs as a Lambda function. When `AWS_LAMBDA_RUNTIME_API`
is set, its entry point takes each invocation's payload,
`{"url": "...", "token": "..."}`, runs a worker, and returns the
worker's JSON report. The worker takes no new part once two thirds of
the time left have passed, so the part it has can finish before the
function's limit; `GENEVA_STOP_MARGIN` sets the seconds kept free
instead. A part that is cut off anyway goes to another worker after the
farm's `--part-timeout`.

Lambda's limits: 15 minutes per invocation, at most 10,240 MB of memory
(which comes with 6 vCPUs) and 10,240 MB of `/tmp`, where the worker
keeps the files it fetches and the part it writes. Functions cannot be
reached from outside, but they can reach out, so the farm needs an
address they can reach, such as a machine with a public address, or one
in the function's VPC.

```sh
aws ecr create-repository --repository-name geneva
docker buildx build --platform linux/arm64 --provenance=false --push \
  -t 123456789012.dkr.ecr.us-east-1.amazonaws.com/geneva:latest .
aws lambda create-function --function-name geneva-worker \
  --package-type Image --architectures arm64 \
  --code ImageUri=123456789012.dkr.ecr.us-east-1.amazonaws.com/geneva:latest \
  --role arn:aws:iam::123456789012:role/geneva-worker \
  --memory-size 10240 --timeout 900 --ephemeral-storage Size=10240
# A failed worker is not worth retrying: the farm gives its part to another.
aws lambda put-function-event-invoke-config --function-name geneva-worker \
  --maximum-retry-attempts 0
```

The role needs no more than `AWSLambdaBasicExecutionRole`, for the logs.
Then:

```sh
geneva farm talk.json -o out.mp4 --local 0 --launch-count 20 \
  --listen 0.0.0.0:8700 --url http://203.0.113.7:8700 \
  --launch 'aws lambda invoke --function-name geneva-worker \
    --invocation-type Event --cli-binary-format raw-in-base64-out \
    --payload "{\"url\":\"$GENEVA_FARM_URL\",\"token\":\"$GENEVA_FARM_TOKEN\"}" /dev/null'
```

## What has been tried

The farm and its local workers are covered by the tests. Workers in a
container started by `--launch 'docker run ...'`, and the Lambda entry
point under AWS's runtime interface emulator, were run while writing
this page; the entry point has not run on Lambda itself. The SSH,
Kubernetes and ECS commands have not been run; they use each tool's
documented flags and nothing of geneva beyond the worker's environment
variables.
