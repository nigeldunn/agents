import {
  Stack,
  StackProps,
  CfnOutput,
  RemovalPolicy,
  Duration,
  aws_ec2 as ec2,
  aws_ecr as ecr,
  aws_ecs as ecs,
  aws_elasticloadbalancingv2 as elbv2,
  aws_iam as iam,
  aws_dynamodb as dynamodb,
  aws_secretsmanager as secretsmanager,
  aws_bedrockagentcore as agentcore,
} from 'aws-cdk-lib';
import { Construct } from 'constructs';

export interface AgentcoreStackProps extends StackProps {
  readonly vpcId: string;
  readonly orchestratorSgId: string;
  readonly clusterName?: string;
  readonly createVpcEndpoints?: boolean;
  /** Image tag the GitHub Actions workflow pushed to both ECR repos. */
  readonly imageTag: string;
}

export class AgentcoreStack extends Stack {
  constructor(scope: Construct, id: string, props: AgentcoreStackProps) {
    super(scope, id, props);

    const vpc = ec2.Vpc.fromLookup(this, 'Vpc', { vpcId: props.vpcId });

    const orchestratorSg = ec2.SecurityGroup.fromSecurityGroupId(
      this,
      'OrchestratorSg',
      props.orchestratorSgId,
      { mutable: false },
    );

    const mockAgentRepo = ecr.Repository.fromRepositoryName(
      this,
      'MockAgentRepo',
      'agents/mock-agent-http',
    );
    const bridgeRepo = ecr.Repository.fromRepositoryName(
      this,
      'BridgeRepo',
      'agents/agentcore-bridge',
    );

    const mockAgentImageUri = `${mockAgentRepo.repositoryUri}:${props.imageTag}`;

    const bearerSecret = new secretsmanager.Secret(this, 'BearerToken', {
      secretName: 'agents/bearer-token',
      description: 'Bearer token gating the AgentCore HTTP bridge',
      generateSecretString: {
        excludePunctuation: true,
        passwordLength: 48,
      },
      removalPolicy: RemovalPolicy.DESTROY,
    });

    const statusTable = new dynamodb.Table(this, 'StatusTable', {
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'ttl',
      removalPolicy: RemovalPolicy.DESTROY,
    });

    const agentCoreRole = new iam.Role(this, 'AgentCoreExecutionRole', {
      assumedBy: new iam.ServicePrincipal('bedrock-agentcore.amazonaws.com'),
      description: 'Execution role for the AgentCore Runtime hosting mock-agent-http',
    });
    mockAgentRepo.grantPull(agentCoreRole);
    agentCoreRole.addToPolicy(new iam.PolicyStatement({
      actions: ['ecr:GetAuthorizationToken'],
      resources: ['*'],
    }));
    agentCoreRole.addToPolicy(new iam.PolicyStatement({
      actions: [
        'logs:CreateLogGroup',
        'logs:CreateLogStream',
        'logs:PutLogEvents',
      ],
      resources: [`arn:${this.partition}:logs:${this.region}:${this.account}:log-group:/aws/bedrock-agentcore/*`],
    }));
    agentCoreRole.addToPolicy(new iam.PolicyStatement({
      actions: ['xray:PutTraceSegments', 'xray:PutTelemetryRecords'],
      resources: ['*'],
    }));
    agentCoreRole.addToPolicy(new iam.PolicyStatement({
      actions: ['cloudwatch:PutMetricData'],
      resources: ['*'],
      conditions: {
        StringEquals: { 'cloudwatch:namespace': 'bedrock-agentcore' },
      },
    }));

    const runtime = new agentcore.CfnRuntime(this, 'Runtime', {
      agentRuntimeName: 'mockAgents',
      roleArn: agentCoreRole.roleArn,
      agentRuntimeArtifact: {
        containerConfiguration: {
          containerUri: mockAgentImageUri,
        },
      },
      networkConfiguration: {
        networkMode: 'PUBLIC',
      },
      protocolConfiguration: 'HTTP',
    });
    runtime.node.addDependency(agentCoreRole);

    const cluster = props.clusterName
      ? ecs.Cluster.fromClusterAttributes(this, 'BridgeCluster', {
          clusterName: props.clusterName,
          vpc,
          securityGroups: [],
        })
      : new ecs.Cluster(this, 'BridgeCluster', { vpc });

    const taskDef = new ecs.FargateTaskDefinition(this, 'BridgeTaskDef', {
      cpu: 512,
      memoryLimitMiB: 1024,
      runtimePlatform: {
        cpuArchitecture: ecs.CpuArchitecture.ARM64,
        operatingSystemFamily: ecs.OperatingSystemFamily.LINUX,
      },
    });

    taskDef.taskRole.addToPrincipalPolicy(new iam.PolicyStatement({
      actions: ['bedrock-agentcore:InvokeAgentRuntime'],
      resources: [
        runtime.attrAgentRuntimeArn,
        `${runtime.attrAgentRuntimeArn}/runtime-endpoint/*`,
      ],
    }));
    statusTable.grantReadWriteData(taskDef.taskRole);
    bearerSecret.grantRead(taskDef.taskRole);

    taskDef.addContainer('bridge', {
      image: ecs.ContainerImage.fromEcrRepository(bridgeRepo, props.imageTag),
      logging: ecs.LogDrivers.awsLogs({
        streamPrefix: 'agentcore-bridge',
      }),
      portMappings: [{ containerPort: 8080, protocol: ecs.Protocol.TCP }],
      environment: {
        AGENTCORE_RUNTIME_ARN: runtime.attrAgentRuntimeArn,
        AGENTCORE_QUALIFIER: 'DEFAULT',
        STATUS_TABLE_NAME: statusTable.tableName,
        BRIDGE_LISTEN_ADDR: '0.0.0.0:8080',
      },
      secrets: {
        AGENTS_BEARER_TOKEN: ecs.Secret.fromSecretsManager(bearerSecret),
      },
    });

    const bridgeSg = new ec2.SecurityGroup(this, 'BridgeServiceSg', {
      vpc,
      description: 'AgentCore bridge Fargate tasks',
      allowAllOutbound: true,
    });
    bridgeSg.addIngressRule(
      orchestratorSg,
      ec2.Port.tcp(8080),
      'Orchestrator to bridge',
    );

    const albSg = new ec2.SecurityGroup(this, 'BridgeAlbSg', {
      vpc,
      description: 'Internal ALB fronting the AgentCore bridge',
      allowAllOutbound: true,
    });
    albSg.addIngressRule(
      orchestratorSg,
      ec2.Port.tcp(80),
      'Orchestrator to ALB',
    );
    bridgeSg.addIngressRule(albSg, ec2.Port.tcp(8080), 'ALB to bridge');

    const service = new ecs.FargateService(this, 'BridgeService', {
      cluster,
      taskDefinition: taskDef,
      desiredCount: 2,
      securityGroups: [bridgeSg],
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_ISOLATED },
      assignPublicIp: false,
      enableExecuteCommand: true,
      minHealthyPercent: 50,
      maxHealthyPercent: 200,
      circuitBreaker: { rollback: true },
    });

    const alb = new elbv2.ApplicationLoadBalancer(this, 'BridgeAlb', {
      vpc,
      internetFacing: false,
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_ISOLATED },
      securityGroup: albSg,
    });

    const listener = alb.addListener('Http', { port: 80, open: false });
    listener.addTargets('BridgeTargets', {
      port: 8080,
      protocol: elbv2.ApplicationProtocol.HTTP,
      targets: [service],
      healthCheck: {
        path: '/healthz',
        port: '8080',
        healthyHttpCodes: '200',
        interval: Duration.seconds(15),
        timeout: Duration.seconds(5),
      },
      deregistrationDelay: Duration.seconds(15),
    });

    if (props.createVpcEndpoints) {
      vpc.addInterfaceEndpoint('BedrockAgentCoreEndpoint', {
        service: new ec2.InterfaceVpcEndpointService(
          `com.amazonaws.${this.region}.bedrock-agentcore`,
          443,
        ),
        privateDnsEnabled: true,
      });
      vpc.addGatewayEndpoint('DynamoDbEndpoint', {
        service: ec2.GatewayVpcEndpointAwsService.DYNAMODB,
      });
      vpc.addInterfaceEndpoint('SecretsManagerEndpoint', {
        service: ec2.InterfaceVpcEndpointAwsService.SECRETS_MANAGER,
      });
      vpc.addInterfaceEndpoint('EcrApiEndpoint', {
        service: ec2.InterfaceVpcEndpointAwsService.ECR,
      });
      vpc.addInterfaceEndpoint('EcrDkrEndpoint', {
        service: ec2.InterfaceVpcEndpointAwsService.ECR_DOCKER,
      });
      vpc.addGatewayEndpoint('S3Endpoint', {
        service: ec2.GatewayVpcEndpointAwsService.S3,
      });
      vpc.addInterfaceEndpoint('LogsEndpoint', {
        service: ec2.InterfaceVpcEndpointAwsService.CLOUDWATCH_LOGS,
      });
    }

    new CfnOutput(this, 'BridgeAlbDns', {
      value: alb.loadBalancerDnsName,
      description: 'Internal ALB DNS for the orchestrator to call',
    });
    new CfnOutput(this, 'AgentCoreRuntimeArn', {
      value: runtime.attrAgentRuntimeArn,
      description: 'AgentCore Runtime ARN (bridge invokes this)',
    });
    new CfnOutput(this, 'StatusTableName', {
      value: statusTable.tableName,
      description: 'DynamoDB table holding cached AgentRunResponses',
    });
    new CfnOutput(this, 'BearerSecretArn', {
      value: bearerSecret.secretArn,
      description: 'Secrets Manager ARN for the shared bearer token',
    });
  }
}
