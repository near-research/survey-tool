export interface FormQuestion {
  id: string;
  section: number;
  section_title: string;
  label: string;
  type: 'single_select' | 'multi_select' | 'rank' | 'open_text' | 'contact';
  options: string[] | null;
  optional: boolean;
  show_if: { question_id: string; value?: string; values?: string[] } | null;
  exclusive_options?: string[];
  rank_count?: number;
}

export interface FormData {
  id: string;
  title: string;
  questions: FormQuestion[];
  creator_id: string;
}
