import type { ClusterFailure } from './api';

const checks: Record<ClusterFailure['check'], string> = {
  configuration: 'Your kubeconfig could not be read.', credentials: "Couldn't sign in to this cluster.", clusterApi: "Couldn't reach this cluster.",
  compatibility: "Couldn't confirm this Orka installation is supported.", serviceAccount: "Couldn't use the service account.",
  permissions: "Couldn't confirm your access to this cluster.", orkaIdentity: "Couldn't verify your account and namespace.", orkaConnection: "Couldn't connect to Orka.",
  agents: "Couldn't read agents from this cluster.", providers: "Couldn't read AI connections from this cluster.",
};
const results: Record<ClusterFailure['result'], string> = {
  unknown: 'The reason is unknown.', connectionFailed: 'The connection could not be opened.',
  authenticationRejected: 'The sign-in was rejected.', permissionDenied: 'Access was denied.',
  unsupportedCredential: 'This credential method could not be used.', incompatible: 'The Orka schema or watch namespace is incompatible.',
  identityRejected: 'The cluster rejected the account or namespace.', noProviders: 'No AI connections are configured on this cluster.',
};
export function clusterRecovery(failure: ClusterFailure) {
  const { check, result } = failure;
  if (check === 'orkaConnection' && result === 'identityRejected') {
    return {
      cause: 'The Orka destination could not be verified. Bridge did not send the Orka credential.',
      next: 'Choose your cluster again in Clusters. Ask its administrator to check the Service, Pod and controller ownership before retrying.',
    };
  }
  const next = check === 'configuration' ? 'Correct your kubeconfig, then press Reload kubeconfig.'
    : check === 'credentials' || result === 'unsupportedCredential' ? 'Check its sign-in method and credential plugin outside Bridge. If you change kubeconfig, press Reload kubeconfig, select the context and use the cluster again. Otherwise, check again.'
    : result === 'connectionFailed' ? 'Check connectivity and any VPN you use, then check again.'
    : result === 'authenticationRejected' ? 'Refresh the sign-in outside Bridge, then press Reload kubeconfig, select the context and use the cluster again.'
    : result === 'permissionDenied' ? 'Ask your cluster administrator to check access for this check, then check again.'
    : check === 'compatibility' ? 'Ask your cluster administrator to check the Orka schema and watched namespace, then check again.'
    : check === 'serviceAccount' ? 'Check the existing ServiceAccount name and token-request access with your cluster administrator, then check again.'
    : check === 'orkaIdentity' ? 'Ask your cluster administrator to check this Orka identity and watched namespace, then check again.'
    : check === 'providers' && result === 'noProviders' ? 'Ask your cluster administrator to configure an AI connection, then check again.'
    : 'Check again. If it still fails, copy the details for help.';
  return { cause: check === 'configuration' ? checks.configuration : `${checks[check]} ${results[result]}`, next };
}
export function clusterDetails(failure: ClusterFailure, operation: string) {
  return ['Bridge cluster connection', `Operation: ${operation}`, `Check: ${failure.check}`, `Result: ${failure.result}`].join('\n');
}
