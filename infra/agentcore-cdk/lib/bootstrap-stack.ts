import {
  Stack,
  StackProps,
  CfnOutput,
  Duration,
  RemovalPolicy,
  aws_iam as iam,
  aws_ecr as ecr,
} from 'aws-cdk-lib';
import { Construct } from 'constructs';

export interface BootstrapStackProps extends StackProps {
  /** GitHub `owner/repo` slug allowed to assume the deploy role. */
  readonly githubRepo: string;
  /** Branch ref allowed to assume the role; defaults to `refs/heads/main`. */
  readonly githubRef?: string;
}

/**
 * One-shot stack deployed manually with local AWS credentials before any CI
 * runs. Provisions:
 *   - The GitHub Actions OIDC provider (idempotent per account).
 *   - An IAM role GitHub Actions assumes via OIDC for builds + deploys.
 *   - Two ECR repositories the workflow pushes container images to.
 *
 * The main `AgentcoreStack` imports the ECR repos by name and references the
 * deploy role only indirectly (via CDK's bootstrap roles, which the GitHub
 * Actions role is allowed to assume).
 */
export class AgentcoreBootstrapStack extends Stack {
  public readonly deployRoleArn: string;

  constructor(scope: Construct, id: string, props: BootstrapStackProps) {
    super(scope, id, props);

    const githubRef = props.githubRef ?? 'refs/heads/main';

    // ECR repos are kept out of stack ownership so they survive stack
    // deletion. CDK manages references only. Apply image scanning + lifecycle
    // policy out-of-band (see README).
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

    // The GitHub Actions OIDC provider is account-wide. If another stack has
    // already created it, set `existingOidcProviderArn` (context) to that ARN
    // to import it instead of failing.
    const existingOidcArn = this.node.tryGetContext('existingOidcProviderArn');
    const oidcProvider: iam.IOpenIdConnectProvider = existingOidcArn
      ? iam.OpenIdConnectProvider.fromOpenIdConnectProviderArn(
          this,
          'GithubOidc',
          existingOidcArn,
        )
      : new iam.OpenIdConnectProvider(this, 'GithubOidc', {
          url: 'https://token.actions.githubusercontent.com',
          clientIds: ['sts.amazonaws.com'],
        });

    const deployRole = new iam.Role(this, 'GithubActionsDeployRole', {
      roleName: 'agents-github-actions-deploy',
      description: 'Assumed by GitHub Actions to build images and deploy the AgentCore stack',
      maxSessionDuration: Duration.hours(1),
      assumedBy: new iam.FederatedPrincipal(
        oidcProvider.openIdConnectProviderArn,
        {
          StringEquals: {
            'token.actions.githubusercontent.com:aud': 'sts.amazonaws.com',
          },
          StringLike: {
            'token.actions.githubusercontent.com:sub': `repo:${props.githubRepo}:ref:${githubRef}`,
          },
        },
        'sts:AssumeRoleWithWebIdentity',
      ),
    });

    mockAgentRepo.grantPullPush(deployRole);
    bridgeRepo.grantPullPush(deployRole);

    deployRole.addToPolicy(new iam.PolicyStatement({
      actions: ['ecr:GetAuthorizationToken'],
      resources: ['*'],
    }));

    deployRole.addToPolicy(new iam.PolicyStatement({
      actions: ['sts:AssumeRole'],
      resources: [
        `arn:${this.partition}:iam::${this.account}:role/cdk-hnb659fds-deploy-role-${this.account}-*`,
        `arn:${this.partition}:iam::${this.account}:role/cdk-hnb659fds-file-publishing-role-${this.account}-*`,
        `arn:${this.partition}:iam::${this.account}:role/cdk-hnb659fds-image-publishing-role-${this.account}-*`,
        `arn:${this.partition}:iam::${this.account}:role/cdk-hnb659fds-lookup-role-${this.account}-*`,
      ],
    }));

    deployRole.addToPolicy(new iam.PolicyStatement({
      actions: [
        'cloudformation:DescribeStacks',
        'cloudformation:GetTemplate',
        'ssm:GetParameter',
        'ssm:GetParameters',
      ],
      resources: ['*'],
    }));

    this.deployRoleArn = deployRole.roleArn;

    new CfnOutput(this, 'DeployRoleArn', {
      value: deployRole.roleArn,
      description: 'IAM role ARN GitHub Actions assumes via OIDC',
    });
    new CfnOutput(this, 'MockAgentRepoUri', {
      value: mockAgentRepo.repositoryUri,
      description: 'ECR repo URI for the AgentCore container image',
    });
    new CfnOutput(this, 'BridgeRepoUri', {
      value: bridgeRepo.repositoryUri,
      description: 'ECR repo URI for the bridge container image',
    });
  }
}
