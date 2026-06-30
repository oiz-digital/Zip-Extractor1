import React from "react";
import { Link } from "wouter";
import { useGetTransactions } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ChevronLeft, ChevronRight } from "lucide-react";

export default function Txs() {
  const [page, setPage] = React.useState(0);
  const limit = 20;
  
  const { data: txs, isLoading } = useGetTransactions({ limit, offset: page * limit });

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Transactions</h2>
          <p className="text-muted-foreground">Browse all transactions on the Zebvix network.</p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => setPage(p => Math.max(0, p - 1))} disabled={page === 0}>
            <ChevronLeft className="w-4 h-4 mr-1" /> Prev
          </Button>
          <span className="text-sm font-mono px-2">Page {page + 1}</span>
          <Button variant="outline" size="sm" onClick={() => setPage(p => p + 1)} disabled={!txs || txs.length < limit}>
            Next <ChevronRight className="w-4 h-4 ml-1" />
          </Button>
        </div>
      </div>

      <Card>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Hash</TableHead>
                  <TableHead>Block</TableHead>
                  <TableHead>Age</TableHead>
                  <TableHead>From / To</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="text-right">Value</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {txs?.map((tx) => (
                  <TableRow key={tx.hash}>
                    <TableCell className="font-mono text-xs">
                      <Link href={`/txs/${tx.hash}`} className="text-primary hover:underline">
                        {formatAddress(tx.hash, 8)}
                      </Link>
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      <Link href={`/blocks/${tx.blockNumber}`} className="hover:underline">
                        {tx.blockNumber}
                      </Link>
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">{timeAgo(tx.timestamp)}</TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      <div className="flex flex-col gap-1">
                        <span className="flex gap-2">
                          <span className="w-4 text-center">F</span>
                          <Link href={`/address/${tx.from}`} className="hover:text-primary transition-colors">{formatAddress(tx.from, 6)}</Link>
                        </span>
                        {tx.to && (
                          <span className="flex gap-2">
                            <span className="w-4 text-center">T</span>
                            <Link href={`/address/${tx.to}`} className="hover:text-primary transition-colors">{formatAddress(tx.to, 6)}</Link>
                          </span>
                        )}
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant="outline" className="text-[10px] uppercase">
                        {tx.type}
                      </Badge>
                    </TableCell>
                    <TableCell>
                      <Badge variant={tx.status === 'success' ? 'default' : tx.status === 'failed' ? 'destructive' : 'secondary'} className="text-[10px]">
                        {tx.status}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs text-foreground">
                      {formatNumber(tx.value, 4)} ZBX
                    </TableCell>
                  </TableRow>
                ))}
                {txs?.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={7} className="text-center py-8 text-muted-foreground">
                      No transactions found.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
