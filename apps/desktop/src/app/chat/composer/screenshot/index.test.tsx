// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Radix popover/tooltip positioning reads element sizes; jsdom has no RO.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

vi.stubGlobal('ResizeObserver', ResizeObserverStub)

const notify = vi.fn()
const notifyError = vi.fn()

vi.mock('@/store/notifications', () => ({
  notify: (input: unknown) => notify(input),
  notifyError: (err: unknown, title: unknown) => notifyError(err, title)
}))

// The annotator is canvas-bound (jsdom has no 2d context) — stub it at the
// component boundary and drive its callbacks directly.
let annotatorProps: { onCancel: () => void; onDone: (blob: Blob, notes: string) => void; open: boolean } | null = null

vi.mock('./annotator', () => ({
  ScreenshotAnnotator: (props: { onCancel: () => void; onDone: (b: Blob, n: string) => void; open: boolean }) => {
    annotatorProps = props

    return props.open ? <div data-testid="annotator-open" /> : null
  }
}))

const listWindows = vi.fn()
const captureWindow = vi.fn()
const onAttachImageBlob = vi.fn()
const onInsertText = vi.fn()

function installBridge() {
  ;(window as unknown as { hermesDesktop: unknown }).hermesDesktop = { captureWindow, listWindows }
}

async function renderButton() {
  const { ScreenshotButton } = await import('./index')
  let result: ReturnType<typeof render>

  await act(async () => {
    result = render(<ScreenshotButton onAttachImageBlob={onAttachImageBlob} onInsertText={onInsertText} />)
  })

  return result!
}

beforeEach(() => {
  annotatorProps = null
  listWindows.mockResolvedValue([
    { id: '42', pid: 42, process: 'notepad.exe', title: 'b.txt' },
    { id: '7', pid: 7, process: 'code.exe', title: 'main.ts' },
    { id: '43', pid: 43, process: 'notepad.exe', title: 'a.txt' }
  ])
  captureWindow.mockResolvedValue('QUJD')
  onAttachImageBlob.mockResolvedValue(true)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  delete (window as unknown as { hermesDesktop?: unknown }).hermesDesktop
})

describe('ScreenshotButton', () => {
  it('renders nothing when the capture bridge is unavailable', async () => {
    const { container } = await renderButton()

    expect(container.firstChild).toBeNull()
  })

  it('lists the whole-screen entry plus windows grouped by process', async () => {
    installBridge()
    await renderButton()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Screenshot' }))
    })

    expect(await screen.findByText('Entire screen')).toBeTruthy()
    // notepad.exe group: titles sorted a.txt before b.txt.
    const b = await screen.findByText('b.txt')
    const a = screen.getByText('a.txt')

    expect(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(screen.getByText('main.ts')).toBeTruthy()
    expect(listWindows).toHaveBeenCalledTimes(1)
  })

  it('captures the picked window, then inserts the image and numbered notes', async () => {
    installBridge()
    await renderButton()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Screenshot' }))
    })
    await screen.findByText('a.txt')

    await act(async () => {
      fireEvent.click(screen.getByText('a.txt'))
    })

    expect(captureWindow).toHaveBeenCalledWith('43')
    expect(await screen.findByTestId('annotator-open')).toBeTruthy()

    const blob = new Blob(['png-bytes'], { type: 'image/png' })

    await act(async () => {
      annotatorProps!.onDone(blob, '① fix the button')
    })

    expect(onAttachImageBlob).toHaveBeenCalledWith(blob)
    expect(onInsertText).toHaveBeenCalledWith('① fix the button')
  })

  it('does not touch the draft when the attachment fails', async () => {
    installBridge()
    onAttachImageBlob.mockResolvedValue(false)
    await renderButton()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Screenshot' }))
    })
    await screen.findByText('Entire screen')

    await act(async () => {
      fireEvent.click(screen.getByText('Entire screen'))
    })
    await screen.findByTestId('annotator-open')

    await act(async () => {
      annotatorProps!.onDone(new Blob(['x'], { type: 'image/png' }), '① note')
    })

    expect(onInsertText).not.toHaveBeenCalled()
  })

  it('reports a capture failure and keeps the picker open', async () => {
    installBridge()
    captureWindow.mockRejectedValue(new Error('the window may be minimized'))
    await renderButton()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Screenshot' }))
    })
    await screen.findByText('main.ts')

    await act(async () => {
      fireEvent.click(screen.getByText('main.ts'))
    })

    // Back in the picker, error surfaced, nothing annotated or attached.
    expect(await screen.findByText('Entire screen')).toBeTruthy()
    expect(notifyError).toHaveBeenCalled()
    expect(annotatorProps?.open).toBeFalsy()
    expect(onAttachImageBlob).not.toHaveBeenCalled()
  })

  it('cancel attaches nothing and writes no notes', async () => {
    installBridge()
    await renderButton()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Screenshot' }))
    })
    await screen.findByText('main.ts')

    await act(async () => {
      fireEvent.click(screen.getByText('main.ts'))
    })
    await screen.findByTestId('annotator-open')

    await act(async () => {
      annotatorProps!.onCancel()
    })

    expect(onAttachImageBlob).not.toHaveBeenCalled()
    expect(onInsertText).not.toHaveBeenCalled()
  })
})
