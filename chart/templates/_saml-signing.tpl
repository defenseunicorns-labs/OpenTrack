{{- define "opentrack.samlSigning" -}}
{{- if and .Values.sso.enabled (not (hasKey .Values.sso "_signingKey")) -}}
  {{- $existing := lookup "v1" "Secret" .Release.Namespace .Values.sso.signingSecret -}}
  {{- if and $existing (index $existing.data "tls.key") (index $existing.data "tls.crt") -}}
    {{- $_ := set .Values.sso "_signingKey" (index $existing.data "tls.key" | b64dec) -}}
    {{- $_ := set .Values.sso "_signingCert" (index $existing.data "tls.crt" | b64dec) -}}
  {{- else -}}
    {{- $material := genSelfSignedCert (printf "opentrack.%s" .Values.domain) nil nil 3650 -}}
    {{- $_ := set .Values.sso "_signingKey" $material.Key -}}
    {{- $_ := set .Values.sso "_signingCert" $material.Cert -}}
  {{- end -}}
{{- end -}}
{{- end -}}
