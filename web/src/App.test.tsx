import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App', () => {
  it('renders a semantic shell heading', () => {
    render(<App />);
    expect(
      screen.getByRole('heading', { name: 'Web shell is ready for the Phase 2 SPA.' }),
    ).toBeVisible();
  });
});
