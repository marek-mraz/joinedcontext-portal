import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import { parseModel } from "../models/linkml";
import { Button, Input } from "../../components/ui";

export interface ClassConfig {
  ticked: boolean;
  slots: string[];
  readQ?: string;
  writable?: boolean;
  idPattern?: string;
  scope?: string;
  writeQ?: string;
}

export interface ModelPickerState {
  dataModelName?: string;
  dataModelVersion?: string;
  projectionName: string;
  selectedProjectionRef?: string;
  classes: Record<string, ClassConfig>;
}

export interface ModelPickerProps {
  project: string;
  spaceName: string;
  endpointName: string;
  disabled?: boolean;
  value: ModelPickerState;
  onChange: (next: ModelPickerState) => void;
}

const IDENTITY_SLOTS = ["id", "type"];

export function ModelPicker({
  project,
  spaceName,
  endpointName,
  disabled = false,
  value,
  onChange,
}: ModelPickerProps): JSX.Element {
  const { t } = useTranslation();
  const [isDetached, setIsDetached] = useState(false);

  const modelsQuery = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  const projectionsQuery = useQuery({
    queryKey: queryKeys.list(project, "projections"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "projections" } },
        }),
      ),
  });

  const endpointsQuery = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const matchingModel = useMemo(() => {
    const list = asManifests(modelsQuery.data?.items ?? []);
    return list.find((m) => {
      const sRef = (m.spec as { contextSpaceRef?: string })?.contextSpaceRef;
      return sRef === spaceName;
    });
  }, [modelsQuery.data, spaceName]);

  const rawLinkml = (matchingModel?.spec as { linkml?: string; source?: string })?.linkml;
  const needsSourceFetch =
    Boolean(matchingModel) && (!rawLinkml || !rawLinkml.includes("\n"));

  const sourceQuery = useQuery({
    queryKey: ["datamodels", project, matchingModel?.metadata.name, "source"],
    enabled: Boolean(matchingModel && needsSourceFetch),
    queryFn: async () => {
      const res = await globalThis.fetch(
        new Request(
          `${window.location.origin}/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(matchingModel!.metadata.name)}/source`,
        ),
      );
      if (!res.ok) return "";
      return res.text();
    },
  });

  const modelSource = rawLinkml && rawLinkml.includes("\n") ? rawLinkml : sourceQuery.data ?? "";

  const parsedModel = useMemo(() => {
    if (!modelSource) return null;
    return parseModel(modelSource);
  }, [modelSource]);

  const modelVersion = useMemo(() => {
    const spec = matchingModel?.spec as { version?: string | number } | undefined;
    return spec?.version ? String(spec.version) : "1";
  }, [matchingModel]);

  // Keep dataModel info synchronized in value
  useEffect(() => {
    if (matchingModel && (value.dataModelName !== matchingModel.metadata.name || value.dataModelVersion !== modelVersion)) {
      onChange({
        ...value,
        dataModelName: matchingModel.metadata.name,
        dataModelVersion: modelVersion,
      });
    }
  }, [matchingModel, modelVersion, value, onChange]);

  // Available projections for this space
  const availableProjections = useMemo(() => {
    const list = asManifests(projectionsQuery.data?.items ?? []);
    return list.filter((p) => {
      const sRef = (p.spec as { contextSpaceRef?: string })?.contextSpaceRef;
      return sRef === spaceName;
    });
  }, [projectionsQuery.data, spaceName]);

  // Find endpoints sharing the chosen projection
  const sharingEndpoints = useMemo(() => {
    if (!value.selectedProjectionRef) return [];
    const eps = asManifests(endpointsQuery.data?.items ?? []);
    return eps
      .filter((ep) => {
        const pRef = (ep.spec as { projectionRef?: { name?: string } })?.projectionRef?.name;
        return pRef === value.selectedProjectionRef;
      })
      .map((ep) => ep.metadata.name);
  }, [endpointsQuery.data, value.selectedProjectionRef]);

  const isReadOnly = Boolean(value.selectedProjectionRef && !isDetached);

  const handleSelectProjection = (projName: string) => {
    if (!projName) {
      // Draw new
      setIsDetached(false);
      onChange({
        ...value,
        selectedProjectionRef: undefined,
        projectionName: value.projectionName || endpointName || "projection",
      });
      return;
    }
    const found = availableProjections.find((p) => p.metadata.name === projName);
    if (!found) return;

    setIsDetached(false);
    const spec = found.spec as {
      classes?: Array<{ name: string; slots?: string[] }>;
      filter?: { q?: string };
    };
    const nextClasses: Record<string, ClassConfig> = {};
    for (const c of spec.classes ?? []) {
      nextClasses[c.name] = {
        ticked: true,
        slots: c.slots ?? [],
        readQ: spec.filter?.q,
      };
    }

    onChange({
      ...value,
      selectedProjectionRef: projName,
      projectionName: projName,
      classes: nextClasses,
    });
  };

  const handleDetach = () => {
    setIsDetached(true);
    onChange({
      ...value,
      selectedProjectionRef: undefined,
      projectionName: endpointName || `${value.projectionName}-copy`,
    });
  };

  const toggleClass = (className: string, allNonIdSlots: string[]) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className];
    const isCurrentlyTicked = Boolean(current?.ticked);
    const nextClasses = { ...value.classes };

    if (isCurrentlyTicked) {
      delete nextClasses[className];
    } else {
      nextClasses[className] = {
        ticked: true,
        slots: [...allNonIdSlots],
        writable: current?.writable,
        idPattern: current?.idPattern,
        scope: current?.scope,
        writeQ: current?.writeQ,
        readQ: current?.readQ,
      };
    }
    onChange({ ...value, classes: nextClasses });
  };

  const toggleSlot = (className: string, slotName: string) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className];
    if (!current?.ticked) return;
    const hasSlot = current.slots.includes(slotName);
    const nextSlots = hasSlot
      ? current.slots.filter((s) => s !== slotName)
      : [...current.slots, slotName];
    onChange({
      ...value,
      classes: {
        ...value.classes,
        [className]: {
          ...current,
          slots: nextSlots,
        },
      },
    });
  };

  const updateClassConfig = (className: string, patch: Partial<ClassConfig>) => {
    if (isReadOnly || disabled) return;
    const current = value.classes[className] ?? { ticked: true, slots: [] };
    onChange({
      ...value,
      classes: {
        ...value.classes,
        [className]: {
          ...current,
          ...patch,
        },
      },
    });
  };

  if (modelsQuery.isPending) {
    return <p className="text-caption text-fg-muted">{t("app.loading")}</p>;
  }

  if (!matchingModel) {
    return (
      <div className="rounded border border-border bg-surface-subtle p-3 text-caption text-fg-muted">
        {t("endpoints.picker.noModel")}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3 rounded border border-border bg-surface-subtle p-3">
      <div>
        <h4 className="text-body font-semibold text-fg">{t("endpoints.picker.title")}</h4>
        <p className="text-caption text-fg-muted">{t("endpoints.picker.hint")}</p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor="projection-reuse" className="text-caption font-medium text-fg">
          {t("endpoints.picker.reuse")}:
        </label>
        <select
          id="projection-reuse"
          disabled={disabled}
          value={value.selectedProjectionRef ?? ""}
          onChange={(e) => handleSelectProjection(e.target.value)}
          className="rounded border border-border bg-surface px-2.5 py-1 text-caption text-fg"
        >
          <option value="">{t("endpoints.picker.drawNew")}</option>
          {availableProjections.map((p) => (
            <option key={p.metadata.name} value={p.metadata.name}>
              {p.metadata.name}
            </option>
          ))}
        </select>

        {value.selectedProjectionRef ? (
          <div className="flex flex-wrap items-center gap-2">
            {sharingEndpoints.length > 0 ? (
              <span className="text-caption text-fg-muted">
                {t("endpoints.picker.sharedBy", { endpoints: sharingEndpoints.join(", ") })}
              </span>
            ) : null}
            {!isDetached ? (
              <Button size="sm" onClick={handleDetach}>
                {t("endpoints.picker.detach")}
              </Button>
            ) : null}
          </div>
        ) : (
          <div className="flex items-center gap-2">
            <label htmlFor="projection-name" className="text-caption font-medium text-fg">
              {t("endpoints.picker.projectionName")}:
            </label>
            <Input
              id="projection-name"
              disabled={disabled || isReadOnly}
              value={value.projectionName}
              onChange={(e) => onChange({ ...value, projectionName: e.target.value })}
              className="h-8 font-mono text-caption"
            />
          </div>
        )}
      </div>

      <div className="flex flex-col gap-3 divide-y divide-border">
        {(parsedModel?.classes ?? []).map((cls) => {
          const cfg = value.classes[cls.name];
          const isTicked = Boolean(cfg?.ticked);
          const nonIdSlots = cls.slots.filter((s) => !IDENTITY_SLOTS.includes(s));
          const isIdentityOnly = isTicked && cfg.slots.length === 0;

          return (
            <div key={cls.name} className="pt-2 flex flex-col gap-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <label className="flex items-center gap-2 font-medium text-caption text-fg cursor-pointer">
                  <input
                    type="checkbox"
                    aria-label={cls.name}
                    disabled={disabled || isReadOnly}
                    checked={isTicked}
                    onChange={() => toggleClass(cls.name, nonIdSlots)}
                  />
                  <span className="font-mono">{cls.name}</span>
                  {isIdentityOnly ? (
                    <span className="text-xs text-fg-muted">({t("endpoints.picker.identityOnly")})</span>
                  ) : null}
                </label>

                {isTicked ? (
                  <div className="flex flex-wrap items-center gap-3">
                    <label className="flex items-center gap-1.5 text-caption text-fg cursor-pointer">
                      <input
                        type="checkbox"
                        aria-label={`${cls.name} writable`}
                        disabled={disabled || isReadOnly}
                        checked={Boolean(cfg.writable)}
                        onChange={(e) => updateClassConfig(cls.name, { writable: e.target.checked })}
                      />
                      <span>{t("endpoints.picker.writable")}</span>
                    </label>

                    <div className="flex items-center gap-1">
                      <input
                        aria-label={`${cls.name} read q`}
                        placeholder={t("endpoints.picker.readQ")}
                        disabled={disabled || isReadOnly}
                        value={cfg.readQ ?? ""}
                        onChange={(e) => updateClassConfig(cls.name, { readQ: e.target.value })}
                        className="rounded border border-border bg-surface px-2 py-0.5 text-caption font-mono text-fg w-32"
                      />
                    </div>
                  </div>
                ) : null}
              </div>

              {isTicked && cfg.writable ? (
                <div className="ml-6 flex flex-wrap gap-2 p-2 bg-surface rounded border border-border">
                  <input
                    aria-label={`${cls.name} idPattern`}
                    placeholder={t("endpoints.picker.idPattern")}
                    disabled={disabled || isReadOnly}
                    value={cfg.idPattern ?? ""}
                    onChange={(e) => updateClassConfig(cls.name, { idPattern: e.target.value })}
                    className="rounded border border-border bg-surface px-2 py-0.5 text-caption font-mono text-fg flex-1 min-w-[12rem]"
                  />
                  <input
                    aria-label={`${cls.name} scope`}
                    placeholder={t("endpoints.picker.scope")}
                    disabled={disabled || isReadOnly}
                    value={cfg.scope ?? ""}
                    onChange={(e) => updateClassConfig(cls.name, { scope: e.target.value })}
                    className="rounded border border-border bg-surface px-2 py-0.5 text-caption font-mono text-fg w-36"
                  />
                  <input
                    aria-label={`${cls.name} q`}
                    placeholder={t("endpoints.picker.q")}
                    disabled={disabled || isReadOnly}
                    value={cfg.writeQ ?? ""}
                    onChange={(e) => updateClassConfig(cls.name, { writeQ: e.target.value })}
                    className="rounded border border-border bg-surface px-2 py-0.5 text-caption font-mono text-fg w-36"
                  />
                </div>
              ) : null}

              {isTicked ? (
                <div className="ml-6 flex flex-wrap gap-x-4 gap-y-1">
                  {IDENTITY_SLOTS.map((idSlot) => (
                    <label key={idSlot} className="flex items-center gap-1.5 text-caption text-fg-muted opacity-75">
                      <input
                        type="checkbox"
                        aria-label={`${cls.name}.${idSlot}`}
                        checked={true}
                        disabled={true}
                      />
                      <span className="font-mono">{idSlot}</span>
                    </label>
                  ))}
                  {nonIdSlots.map((slot) => {
                    const slotTicked = cfg.slots.includes(slot);
                    return (
                      <label key={slot} className="flex items-center gap-1.5 text-caption text-fg cursor-pointer">
                        <input
                          type="checkbox"
                          aria-label={`${cls.name}.${slot}`}
                          disabled={disabled || isReadOnly}
                          checked={slotTicked}
                          onChange={() => toggleSlot(cls.name, slot)}
                        />
                        <span className="font-mono">{slot}</span>
                      </label>
                    );
                  })}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
