import type { ReactNode } from 'react'
import {
  TbAddressBook,
  TbApi,
  TbArrowsExchange,
  TbBan,
  TbBroadcast,
  TbClockPause,
  TbFile,
  TbFilter,
  TbFlag,
  TbMessages,
  TbNetwork,
  TbPlugConnected,
  TbRadar,
  TbRoute,
  TbSend,
  TbServer,
  TbTopologyStar,
  TbWorldDownload,
} from 'react-icons/tb'
import type { SourceSpec } from '../../../api/client'
import type { StepType } from './sourceFlow'

const TRANSPORT_ICONS: Record<string, ReactNode> = {
  tcp_client: <TbNetwork />,
  tcp_server: <TbServer />,
  udp: <TbBroadcast />,
  http_poll: <TbWorldDownload />,
  websocket: <TbPlugConnected />,
  mqtt: <TbMessages />,
  grpc_client: <TbApi />,
  grpc_server: <TbArrowsExchange />,
  file: <TbFile />,
}

const STEP_ICONS: Record<Exclude<StepType, 'input'>, ReactNode> = {
  reject: <TbBan />,
  map: <TbRoute />,
  filter: <TbFilter />,
  registry: <TbAddressBook />,
  affiliation: <TbFlag />,
  tracker: <TbRadar />,
  throttle: <TbClockPause />,
  output: <TbSend />,
  nats: <TbTopologyStar />,
}

/** A step's icon: the input's is its transport's. */
export function stepIcon(spec: SourceSpec, type: StepType): ReactNode {
  return type === 'input' ? TRANSPORT_ICONS[spec.transport.type] : STEP_ICONS[type]
}
