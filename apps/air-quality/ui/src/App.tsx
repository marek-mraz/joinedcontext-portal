import { useCallback, useEffect, useState } from "react";
import type { JSX } from "react";
import { ApiError, getIdentity, getStations, writeNote } from "./api";
import type { Identity, Station } from "./api";

/**
 * The whole app: who you are, what the stations read, and a note box for the people the
 * gateway lets write. The note box follows `canWriteNote`, which the server got from the
 * PDP, so what is hidden here is also refused there.
 */
export function App(): JSX.Element {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [stations, setStations] = useState<Station[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [me, list] = await Promise.all([getIdentity(), getStations()]);
      setIdentity(me);
      setStations(list);
      setError(null);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.detail : String(cause));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main>
      <h1>Air quality</h1>
      <p className="identity">{who(identity)}</p>

      {error && <p role="alert">{error}</p>}
      {!stations && !error && <p role="status">Loading stations…</p>}

      {stations && stations.length === 0 && <p>No stations here yet.</p>}

      {stations && stations.length > 0 && (
        <ul className="stations">
          {stations.map((station) => (
            <StationCard
              key={station.id}
              station={station}
              mayWrite={identity?.canWriteNote ?? false}
              onSaved={load}
            />
          ))}
        </ul>
      )}
    </main>
  );
}

function who(identity: Identity | null): string {
  if (!identity) {
    return "…";
  }
  return identity.signedIn
    ? `Signed in as ${identity.email ?? identity.user ?? "unknown"}`
    : "You are viewing anonymously.";
}

function StationCard({
  station,
  mayWrite,
  onSaved,
}: {
  station: Station;
  mayWrite: boolean;
  onSaved: () => Promise<void>;
}): JSX.Element {
  return (
    <li className="station">
      <h2>{station.name ?? station.id}</h2>
      <dl>
        <Metric label="PM10" value={station.pm10} unit="µg/m³" />
        <Metric label="PM2.5" value={station.pm25} unit="µg/m³" />
        <Metric label="Index" value={station.airQualityIndex} />
      </dl>
      {station.observedAt && <p className="identity">Measured: {station.observedAt}</p>}
      {station.stewardNote && <p>Note: {station.stewardNote}</p>}
      {mayWrite && <NoteBox station={station} onSaved={onSaved} />}
    </li>
  );
}

function Metric({
  label,
  value,
  unit,
}: {
  label: string;
  value: number | undefined;
  unit?: string;
}): JSX.Element | null {
  // An attribute the grant hides never arrives, and an empty row would suggest a zero.
  if (value === undefined) {
    return null;
  }
  return (
    <>
      <dt>{label}</dt>
      <dd>
        {value}
        {unit ? ` ${unit}` : ""}
      </dd>
    </>
  );
}

function NoteBox({
  station,
  onSaved,
}: {
  station: Station;
  onSaved: () => Promise<void>;
}): JSX.Element {
  const [note, setNote] = useState("");
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const label = `Steward note for ${station.name ?? station.id}`;

  return (
    <form
      className="note"
      onSubmit={(event) => {
        event.preventDefault();
        setSaving(true);
        setProblem(null);
        writeNote(station.id, note)
          .then(() => {
            setNote("");
            return onSaved();
          })
          // The gateway's refusal is the message: it names the role that is missing.
          .catch((cause: unknown) => {
            setProblem(cause instanceof ApiError ? cause.detail : String(cause));
          })
          .finally(() => {
            setSaving(false);
          });
      }}
    >
      <label htmlFor={`note-${station.id}`}>{label}</label>
      <textarea
        id={`note-${station.id}`}
        rows={2}
        maxLength={500}
        value={note}
        onChange={(event) => {
          setNote(event.target.value);
        }}
      />
      <button type="submit" disabled={saving || note.trim().length === 0}>
        Save note
      </button>
      {problem && <p role="alert">{problem}</p>}
    </form>
  );
}
