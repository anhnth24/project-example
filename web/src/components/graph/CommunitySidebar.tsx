// P2-17 Document Graph MVP sidebar: one checkbox per community + its node
// count, plus "Select All" — matches the owner's reference screenshot
// ("Communities" checkbox list with a count per cluster).
import type { components } from '../../api/generated/contract';

type GraphCommunity = components['schemas']['GraphCommunity'];

export function CommunitySidebar({
  communities,
  hiddenCommunityIds,
  onToggle,
  onSelectAll,
}: {
  communities: GraphCommunity[];
  hiddenCommunityIds: ReadonlySet<string>;
  onToggle: (communityId: string) => void;
  onSelectAll: () => void;
}) {
  const allVisible = communities.every((c) => !hiddenCommunityIds.has(c.id));
  return (
    <section className="graph-sidebar" aria-labelledby="graph-communities-heading">
      <div className="graph-sidebar-header">
        <h2 id="graph-communities-heading">Cộng đồng</h2>
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          onClick={onSelectAll}
          disabled={allVisible}
        >
          Chọn tất cả
        </button>
      </div>
      {communities.length === 0 ? (
        <p className="text-muted">Không có cụm nào.</p>
      ) : (
        <ul className="graph-community-list">
          {communities.map((community) => {
            const inputId = `graph-community-${community.id}`;
            const checked = !hiddenCommunityIds.has(community.id);
            return (
              <li key={community.id}>
                <label htmlFor={inputId} className="graph-community-item">
                  <input
                    id={inputId}
                    type="checkbox"
                    checked={checked}
                    onChange={() => onToggle(community.id)}
                  />
                  <span className="graph-community-label">{community.label}</span>
                  <span className="graph-community-count">{community.size}</span>
                </label>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
