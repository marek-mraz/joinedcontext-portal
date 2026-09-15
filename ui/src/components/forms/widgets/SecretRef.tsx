import { useState, useEffect } from "react";
import type { ChangeEvent, JSX } from "react";
import type { WidgetProps } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import React from "react";

export interface SecretRefValue {
  name: string;
  key?: string;
  envVar?: string;
}

export interface SecretRefContextType {
  knownSecretNames?: string[];
  onSecretRef?: (envVar: string, ref: SecretRefValue) => void;
  secrets?: SecretRefValue[];
}

export const SecretRefContext = React.createContext<SecretRefContextType>({});

function sanitizePart(s: string): string {
  return s.replace(/[^a-zA-Z0-9]/g, "_").toUpperCase();
}

/**
 * Widget for fields marked `secret: true` in Bento inputs (MF-35, PL-16, PL-50).
 *
 * Renders two inputs: secret name (with a datalist of existing secret references in the project)
 * and key (default "password" or the field's last path segment). Its value in the form is
 * `${DS_<NAME>_<KEY>}`, and it notifies `onSecretRef` so the page collects `spec.secrets`.
 */
export function SecretRefWidget(props: WidgetProps): JSX.Element {
  const {
    id,
    value,
    disabled,
    readonly,
    onChange,
    name,
    options,
    formContext,
    rawErrors,
  } = props;

  const { t } = useTranslation();
  const context = React.useContext(SecretRefContext);

  const knownSecrets: string[] =
    (options?.knownSecretNames as string[]) ||
    formContext?.knownSecretNames ||
    context.knownSecretNames ||
    [];

  const onSecretRef = formContext?.onSecretRef || context.onSecretRef;
  const existingSecrets: SecretRefValue[] =
    formContext?.secrets || context.secrets || [];

  const defaultKey = name || "password";

  const initialVar =
    typeof value === "string" ? value.match(/^\$\{([A-Z0-9_]+)\}$/)?.[1] : undefined;
  const matchedSecret = initialVar
    ? existingSecrets.find((s) => s.envVar === initialVar)
    : undefined;

  const [secretName, setSecretName] = useState<string>(() => matchedSecret?.name ?? "");
  const [secretKey, setSecretKey] = useState<string>(() => matchedSecret?.key ?? defaultKey);

  // A manifest opened for editing arrives after the first render: adopt its reference once.
  const wanted = matchedSecret ? `${matchedSecret.name}\n${matchedSecret.key ?? ""}` : "";
  const [adopted, setAdopted] = useState(wanted);
  if (wanted !== adopted) {
    setAdopted(wanted);
    if (matchedSecret) {
      setSecretName(matchedSecret.name);
      if (matchedSecret.key) setSecretKey(matchedSecret.key);
    }
  }

  useEffect(() => {
    const trimmedName = secretName.trim();
    const trimmedKey = secretKey.trim() || defaultKey;
    if (trimmedName) {
      const envVar = `DS_${sanitizePart(trimmedName)}_${sanitizePart(trimmedKey)}`;
      onSecretRef?.(envVar, { name: trimmedName, key: trimmedKey, envVar });
    }
  }, [secretName, secretKey, defaultKey, onSecretRef]);

  const updateValue = (nextName: string, nextKey: string) => {
    const trimmedName = nextName.trim();
    const trimmedKey = nextKey.trim() || defaultKey;
    if (trimmedName) {
      const envVar = `DS_${sanitizePart(trimmedName)}_${sanitizePart(trimmedKey)}`;
      const interpolation = `\${${envVar}}`;
      onChange(interpolation);
      onSecretRef?.(envVar, { name: trimmedName, key: trimmedKey, envVar });
    } else {
      onChange(undefined);
    }
  };

  const handleNameChange = (e: ChangeEvent<HTMLInputElement>) => {
    const next = e.target.value;
    setSecretName(next);
    updateValue(next, secretKey);
  };

  const handleKeyChange = (e: ChangeEvent<HTMLInputElement>) => {
    const next = e.target.value;
    setSecretKey(next);
    updateValue(secretName, next);
  };

  const hasErrors = Boolean(rawErrors && rawErrors.length > 0);
  const datalistId = `${id}__secrets_list`;

  return (
    <div className="space-y-1">
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <div>
          <label htmlFor={`${id}__name`} className="sr-only">
            {t("datasources.secretRef.name", { defaultValue: "Secret name" })}
          </label>
          <input
            id={`${id}__name`}
            type="text"
            list={knownSecrets.length > 0 ? datalistId : undefined}
            disabled={disabled}
            readOnly={readonly}
            placeholder={t("datasources.secretRef.name", { defaultValue: "Secret name" })}
            value={secretName}
            onChange={handleNameChange}
            aria-invalid={hasErrors ? "true" : undefined}
            className={clsx(
              "block w-full rounded border border-border bg-surface px-3 py-1.5 text-sm text-surface-fg placeholder:text-surface-fg/50 focus:border-border-focus focus:outline-none focus:ring-2 focus:ring-border-focus",
              disabled && "cursor-not-allowed opacity-50"
            )}
          />
          {knownSecrets.length > 0 && (
            <datalist id={datalistId}>
              {knownSecrets.map((s) => (
                <option key={s} value={s} />
              ))}
            </datalist>
          )}
        </div>
        <div>
          <label htmlFor={`${id}__key`} className="sr-only">
            {t("datasources.secretRef.key", { defaultValue: "Key" })}
          </label>
          <input
            id={`${id}__key`}
            type="text"
            disabled={disabled}
            readOnly={readonly}
            placeholder={t("datasources.secretRef.key", { defaultValue: "Key (e.g. password)" })}
            value={secretKey}
            onChange={handleKeyChange}
            className={clsx(
              "block w-full rounded border border-border bg-surface px-3 py-1.5 text-sm text-surface-fg placeholder:text-surface-fg/50 focus:border-border-focus focus:outline-none focus:ring-2 focus:ring-border-focus",
              disabled && "cursor-not-allowed opacity-50"
            )}
          />
        </div>
      </div>
      {typeof value === "string" && value.startsWith("${") && (
        <p className="font-mono text-xs text-surface-fg/60" data-testid={`${id}__interpolation`}>
          {value}
        </p>
      )}
    </div>
  );
}

export default SecretRefWidget;
