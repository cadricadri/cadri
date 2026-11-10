import type { Filters } from "./filters";
import { harnessInfo } from "./HarnessLogo";

type Option = { value: string; label: string };

function Select({
  label,
  value,
  allLabel,
  options,
  onChange,
}: {
  label: string;
  value: string | null;
  allLabel: string;
  options: Option[];
  onChange: (value: string | null) => void;
}) {
  const known = value === null || options.some((option) => option.value === value);
  return (
    <label className="filter-select">
      <span className="filter-label">{label}</span>
      <select value={value ?? ""} onChange={(event) => onChange(event.target.value || null)}>
        <option value="">{allLabel}</option>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
        {known ? null : <option value={value ?? ""}>{value}</option>}
      </select>
    </label>
  );
}

export function SessionFilters({
  filters,
  harnesses,
  models,
  users,
  userLabel,
  onChange,
}: {
  filters: Filters;
  harnesses: string[];
  models: string[];
  users: string[];
  userLabel: (userId: string) => string;
  onChange: (changes: Partial<Filters>) => void;
}) {
  return (
    <div className="session-filters">
      <div className="filter-grid">
        <Select
          label="Harness"
          value={filters.harness}
          allLabel="All agent harnesses"
          options={harnesses.map((value) => ({ value, label: harnessInfo(value).label }))}
          onChange={(harness) => onChange({ harness })}
        />
        <Select
          label="Model"
          value={filters.model}
          allLabel="All models"
          options={models.map((value) => ({ value, label: value }))}
          onChange={(model) => onChange({ model })}
        />
        <Select
          label="User"
          value={filters.user}
          allLabel="All users"
          options={users.map((value) => ({ value, label: userLabel(value) }))}
          onChange={(user) => onChange({ user })}
        />
      </div>
    </div>
  );
}
