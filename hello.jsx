// Child component - just forwards the ref to the input
const EnhancedInput = forwardRef(({ label, ...props }, ref) => {
  // directly forward the ref to the input
  return (
    <div className="input-wrapper">
      {label && <label>{label}</label>}
      <input ref={ref} {...props} />
    </div>
  );
});

// Parent component - handles all the logic
export default function HelloPage() {
  const inputRef = useRef(null);
  
  const focusInput = () => {
    inputRef.current?.focus();
  };
  
  const selectInput = () => {
    inputRef.current?.select();
  };
  
  const getValue = () => {
    return inputRef.current?.value;
  };
  
  const clearInput = () => {
    if (inputRef.current) {
      inputRef.current.value = '';
    }
  };
  
  const handleSubmit = () => {
    const value = getValue();
    console.log('Submitting:', value);
    clearInput();
  };
  
  return (
    <div>
      <EnhancedInput 
        ref={inputRef}
        label="Username"
        placeholder="Enter username"
      />
      <button onClick={focusInput}>Focus</button>
      <button onClick={selectInput}>Select All</button>
      <button onClick={handleSubmit}>Submit</button>
    </div>
  );
}