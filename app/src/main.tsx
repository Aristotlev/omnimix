import { createRoot } from 'react-dom/client';
import App from './App';
import { connect } from './api';
import './styles.css';

connect().then((t) => {
  createRoot(document.getElementById('root')!).render(<App t={t} />);
});
